/* SPDX-License-Identifier: MIT
 * Rootful DDX based on XLibre's software framebuffer. No X11 protocol reimplementation.
 * The included upstream file retains its original copyright and license. */
#include <math.h>

#define InitOutput WayxVfbInitOutput
#define ddxGiveUp WayxVfbGiveUp
#define ddxProcessArgument WayxVfbProcessArgument
#include "../vfb/InitOutput.c"
#undef InitOutput
#undef ddxGiveUp
#undef ddxProcessArgument
#include "wayx.h"
#include "dix/input_priv.h"
#include "dix/resource_priv.h"
#include "dix/screen_hooks_priv.h"
#include "dix/window_priv.h"
#include "windowstr.h"
#include "damage.h"
#include "regionstr.h"
#include "inputstr.h"
#include "xkbsrv.h"

void InitOutput(int argc,char **argv);
void ddxGiveUp(enum ExitCode error);
_X_EXPORT int wayx_xserver_run(const WayxApi*,WayxBackend*,int,char**);

static const WayxApi *api;
static WayxBackend *backend;
static DamagePtr damage;
static int input_socket=-1;
static Bool handlers_registered;
static ScreenPtr screen;
extern DeviceIntPtr wayx_pointer, wayx_keyboard;
/* Private XKB paths are pinned to the same XLibre revision as this DDX. */
extern const char *XkbBaseDirectory, *XkbBinDirectory;
#ifdef HAVE_SETITIMER
extern Bool SmartScheduleSignalEnable;
#endif
static WayxDesktopLayout host_layout;
static Bool host_layout_active,layout_applying;
static int32_t desktop_origin_x,desktop_origin_y;
static Bool root_geometry_pending=TRUE;
#include "wayx-rootless.h"
#include "wayx-randr.h"
#include "wayx-signal.h"

/* One X screen with independently addressable RandR outputs, not separate
 * DISPLAY screens. Coordinates are in the shared desktop framebuffer. */
static vfbCrtcInfo monitors[16];
static unsigned monitor_count;
static Bool explicit_screen,explicit_crtcs;
static Bool monitor_argument(const char *text,vfbCrtcInfoPtr monitor) {
    long values[4];const char separators[]={'x','+','+','\0'};
    for (unsigned i=0;i<4;i++) {
        if (*text<'0' || *text>'9') return FALSE;
        char *end;errno=0;
        values[i]=strtol(text,&end,10);
        if (errno || values[i]<0 || values[i]>16384 || *end!=separators[i]) return FALSE;
        text=end+(i<3);
    }
    if (!values[0] || !values[1] || values[0]+values[2]>16384 || values[1]+values[3]>16384)
        return FALSE;
    *monitor=(vfbCrtcInfo){.width=values[0],.height=values[1],.x=values[2],.y=values[3],.numOutputs=1};
    return TRUE;
}

int ddxProcessArgument(int argc,char **argv,int index) {
    if (!strcmp(argv[index],"-screen")) explicit_screen=TRUE;
    if (!strcmp(argv[index],"-crtcs")) explicit_crtcs=TRUE;
    if (!strcmp(argv[index],"-wayx-rootless")) {rootless=TRUE;return 1;}
    if (!strcmp(argv[index],"-wayx-monitor")) {
        if (index+1>=argc || monitor_count>=16 || !monitor_argument(argv[index+1],&monitors[monitor_count]))
            FatalError("WayX: -wayx-monitor expects WxH+X+Y (nonnegative desktop coordinates, at most 16 outputs)\n");
        monitor_count++;return 2;
    }
    return WayxVfbProcessArgument(argc,argv,index);
}


/* USB HID usage -> Linux evdev XKB keycode (+8). No enum/scancode casts. */
static unsigned int keycode(unsigned int hid) {
    static const unsigned char letter_evdev[26]={30,48,46,32,18,33,34,35,23,36,37,38,50,49,24,25,16,19,31,20,22,47,17,45,21,44};
    if (hid>=4 && hid<=29) return letter_evdev[hid-4]+8;
    if (hid>=30 && hid<=38) return hid-30+2+8;
    if (hid==39) return 11+8;
    if (hid>=58 && hid<=67) return hid-58+59+8;
    switch (hid) {
    case 40:return 28+8; case 41:return 1+8; case 42:return 14+8; case 43:return 15+8; case 44:return 57+8;
    case 45:return 12+8; case 46:return 13+8; case 47:return 26+8; case 48:return 27+8; case 49:return 43+8;
    case 51:return 39+8; case 52:return 40+8; case 53:return 41+8; case 54:return 51+8; case 55:return 52+8; case 56:return 53+8; case 57:return 58+8;
    case 68:return 87+8; case 69:return 88+8; case 70:return 99+8; case 71:return 70+8; case 72:return 119+8;
    case 73:return 110+8; case 74:return 102+8; case 75:return 104+8; case 76:return 111+8; case 77:return 107+8; case 78:return 109+8;
    case 79:return 106+8; case 80:return 105+8; case 81:return 108+8; case 82:return 103+8;
    case 83:return 69+8; case 84:return 98+8; case 85:return 55+8; case 86:return 74+8; case 87:return 78+8; case 88:return 96+8;
    case 89:return 79+8; case 90:return 80+8; case 91:return 81+8; case 92:return 75+8; case 93:return 76+8; case 94:return 77+8;
    case 95:return 71+8; case 96:return 72+8; case 97:return 73+8; case 98:return 82+8; case 99:return 83+8;
    case 224:return 29+8;case 225:return 42+8;case 226:return 56+8;case 227:return 125+8;
    case 228:return 97+8;case 229:return 54+8;case 230:return 100+8;case 231:return 126+8;
    default:return 0;
    }
}
static unsigned char keys_down[256];
static unsigned char buttons_down[32];
static void release_input(void) {
    for (unsigned int i=8;i<256;i++) if (keys_down[i]) {QueueKeyboardEvents(wayx_keyboard,KeyRelease,i);keys_down[i]=0;}
    ValuatorMask *mask=valuator_mask_new(0);
    for (unsigned int i=1;i<32;i++) if (buttons_down[i]) {QueuePointerEvents(wayx_pointer,ButtonRelease,i,0,mask);buttons_down[i]=0;}
    valuator_mask_free(&mask);
}
static void button(int which,int pressed) {
    if (which<1 || which>13) return;
    ValuatorMask *mask=valuator_mask_new(0);
    QueuePointerEvents(wayx_pointer,pressed?ButtonPress:ButtonRelease,which,0,mask);
    buttons_down[which]=pressed;
    valuator_mask_free(&mask);
}
/* Native geometry is a window-manager action. serverClient would redirect it
 * back into the asynchronous WM connection and report Success before applying
 * it. Use the redirect owner's context so the final observation is synchronous.
 * Remote X11 requests still take their ordinary ConfigureRequest policy path. */
static int configure_native_window(WindowPtr window,Mask mask,XID *values) {
    ClientPtr manager=serverClient;
    WindowPtr parent=window->parent;
    if (parent) {
        if (parent->eventMask & SubstructureRedirectMask) manager=dixClientForWindow(parent);
        else for (OtherClientsPtr other=wOtherClients(parent);other;other=other->next)
            if (other->mask & SubstructureRedirectMask) {manager=dixClientForOtherClients(other);break;}
    }
    return ConfigureWindow(window,mask,values,manager);
}
static uint32_t root_resize_ack;
static unsigned int root_width,root_height;
static void flush_backend(void) {
    WayxInputEvent event;
    for (int count=0;count<4096 && api->input_poll(backend,&event)==1;count++) {
        WindowPtr target=NULL;
        if (rootless && event.window!=0 && event.window!=1) {
            if (dixLookupWindow(&target,event.window,serverClient,DixReadAccess)!=Success) continue;
        }
        switch (event.kind) {
        case WAYX_MOTION: {
            ValuatorMask *mask=valuator_mask_new(2);
            valuator_mask_set_double(mask,0,event.x+(target?target->drawable.x:(rootless?-(double)desktop_origin_x:0)));
            valuator_mask_set_double(mask,1,event.y+(target?target->drawable.y:(rootless?-(double)desktop_origin_y:0)));
            QueuePointerEvents(wayx_pointer,MotionNotify,0,POINTER_ABSOLUTE|POINTER_SCREEN,mask);
            valuator_mask_free(&mask);break;
        }
        case WAYX_BUTTON:
            if(target && event.flags && !target->overrideRedirect) {
                XID values[]={Above};ConfigureWindow(target,CWStackMode,values,serverClient);
                SetInputFocus(serverClient,GetMaster(wayx_keyboard,MASTER_KEYBOARD),target->drawable.id,RevertToPointerRoot,CurrentTime,TRUE);
            }
            button(event.detail,event.flags!=0);break;
        case WAYX_KEY: {
            unsigned int key=keycode(event.detail);
            if (key) {QueueKeyboardEvents(wayx_keyboard,event.flags?KeyPress:KeyRelease,key);keys_down[key]=event.flags!=0;}
            break;
        }
        case WAYX_SCROLL: {
            ValuatorMask *mask=valuator_mask_new(4);
            /* Match the previous 32-line per-event bound without truncating
             * fractional movement. Direction follows winit, including the OS
             * natural-scrolling preference. Do not synthesize buttons twice. */
            valuator_mask_set_double(mask,2,fmax(-32.0,fmin(event.x,32.0)));
            valuator_mask_set_double(mask,3,fmax(-32.0,fmin(event.y,32.0)));
            QueuePointerEvents(wayx_pointer,MotionNotify,0,POINTER_RELATIVE,mask);
            valuator_mask_free(&mask);break;
        }
        case WAYX_FOCUS:
            if (!event.flags) release_input();
            else if (target) SetInputFocus(serverClient,GetMaster(wayx_keyboard,MASTER_KEYBOARD),target->drawable.id,RevertToPointerRoot,CurrentTime,TRUE);
            break;
        case WAYX_CLOSE:
            if (target) close_window(target);
            else GiveUp(0);
            break;
        case WAYX_RESIZE:
            if (!rootless && event.window==1 && isfinite(event.x) && isfinite(event.y) &&
                event.x>=1 && event.x<=16384 && event.y>=1 && event.y<=16384)
                resize_desktop(event.x,event.y);
            if(target && event.x>=1 && event.x<=16384 && event.y>=1 && event.y<=16384) {
                uint32_t width=event.x,height=event.y;
                if(api->window_constrain(backend,event.window,&width,&height)==0) {
                    /* Geometry acknowledgement must follow this input in the same batch,
                     * including requests constrained back to the existing size. */
                    dirty_top(target,WINDOW_GEOMETRY|WINDOW_PIXELS);
                    XID values[]={width,height};configure_native_window(target,CWWidth|CWHeight,values);
                    native_window_state(target,event.flags);
                }
            }
            break;
        case WAYX_DESKTOP:
            if (rootless) apply_host_layout();
            break;
        case WAYX_MOVE:
            if(target && event.x-desktop_origin_x>=-32768 && event.x-desktop_origin_x<=32767 &&
                event.y-desktop_origin_y>=-32768 && event.y-desktop_origin_y<=32767) {
                int32_t x=event.x-desktop_origin_x,y=event.y-desktop_origin_y;
                /* Even an unchanged/rejected request needs a final geometry ack. */
                dirty_top(target,WINDOW_GEOMETRY|WINDOW_PIXELS);
                if (target->drawable.x-target->borderWidth!=x || target->drawable.y-target->borderWidth!=y) {
                    XID values[]={x,y};configure_native_window(target,CWX|CWY,values);
                }
            }
            break;
        case WAYX_STATE:if(target) native_window_state(target,event.flags);break;
        case WAYX_SHUTDOWN:GiveUp(0);break;
        /* TEXT/CLIPBOARD are consumed
         * by the selection/XIM layer, not synthesized into raw X keycodes. */
        default:break;
        }
        /* Completion is recorded only after the request has been processed.
         * It is paired with a fresh final geometry at the end of this batch. */
        if (event.detail && (event.kind==WAYX_MOVE || event.kind==WAYX_RESIZE)) {
            WayxWindow *tracked=target?find_window(target->drawable.id):NULL;
            if (tracked) {
                if (event.kind==WAYX_MOVE) tracked->move_ack=event.detail;
                else tracked->resize_ack=event.detail;
                tracked->geometry_pending=TRUE;dirty_window(tracked,WINDOW_GEOMETRY);
            } else if (!rootless && event.window==1 && event.kind==WAYX_RESIZE) {
                root_resize_ack=event.detail;root_geometry_pending=TRUE;
            }
        }
    }
    if (rootless && screen->root) sync_rootless_windows();
    if (root_geometry_pending || root_width!=screen->width || root_height!=screen->height) {
        WayxGeometrySnapshot geometry={0};
        geometry.width=screen->width;geometry.height=screen->height;
        geometry.state.x=desktop_origin_x;geometry.state.y=desktop_origin_y;
        geometry.resize_ack=root_resize_ack;
        if (api->window_geometry(backend,1,&geometry)<0) FatalError("WayX root geometry rejected\n");
        root_width=screen->width;root_height=screen->height;root_geometry_pending=FALSE;
    }
    if (!rootless && damage && !RegionNil(DamageRegion(damage))) {
        RegionPtr region=DamageRegion(damage);
        BoxPtr boxes=RegionRects(region);
        vfbScreenInfoPtr fb=vfbGetScreenPriv(screen);
        for (int i=0;i<RegionNumRects(region);i++) {
            BoxRec b=boxes[i];
            if (b.x1<0) b.x1=0;
            if (b.y1<0) b.y1=0;
            if (b.x2>screen->width) b.x2=screen->width;
            if (b.y2>screen->height) b.y2=screen->height;
            if (b.x2<=b.x1 || b.y2<=b.y1) continue;
            size_t stride=fb->paddedBytesWidth;
            size_t len=(b.y2-b.y1-1)*stride+(b.x2-b.x1)*4;
            const uint8_t *pixels=(const uint8_t*)fb->pfbMemory+b.y1*stride+b.x1*4;
            if (api->frame_damage(backend,1,b.x1,b.y1,b.x2-b.x1,b.y2-b.y1,stride,pixels,len)<0)
                FatalError("WayX framebuffer rejected damage\n");
        }
        DamageEmpty(damage);
    }
    if (api->output_flush(backend)<0) FatalError("WayX output notification failed\n");
}
static void backend_block(void *data,void *timeout) { flush_backend(); }
static void backend_wakeup(void *data,int result) { }
static void backend_input(int fd,int ready,void *data) {
    if (api->input_ack(backend)<0) FatalError("WayX input notification failed\n");
    flush_backend();
}
static void register_backend(void) {
    install_signal_wake();
    input_socket=api->input_fd(backend);
    if (input_socket<0 || !SetNotifyFd(input_socket,backend_input,X_NOTIFY_READ,NULL))
        FatalError("WayX could not register input notification socket\n");
    if (!RegisterBlockAndWakeupHandlers(backend_block,backend_wakeup,NULL))
        FatalError("WayX could not register output batch handler\n");
    handlers_registered=TRUE;
}
static void detach_screen(CallbackListPtr *list,ScreenPtr closing,void *data) {
    /* Close callbacks run newest first. Unwrap before Composite/Damage release
     * their screen privates, and before dixFreeScreen frees the Screen itself. */
    dixScreenUnhookClose(closing,detach_screen);
    if (input_socket>=0) {RemoveNotifyFd(input_socket);input_socket=-1;}
    if (handlers_registered) {RemoveBlockAndWakeupHandlers(backend_block,backend_wakeup,NULL);handlers_registered=FALSE;}
    if (rootless) remove_window_hooks();
    if (damage) {DamageDestroy(damage);damage=NULL;}
    while (wayx_windows) {WayxWindow *entry=wayx_windows;wayx_windows=entry->next;remove_window(entry);}
    api->window_destroy(backend,1);
    screen=NULL;
}
static void attach_screen(CallbackListPtr *list,void *closure,void *data) {
    screen=data;
    dixScreenHookClose(screen,detach_screen);
    root_resize_ack=0;root_geometry_pending=TRUE;root_width=0;root_height=0;
    vfbScreenInfoPtr fb=vfbGetScreenPriv(screen);
    if (screen->myNum!=0 || fb->bitsPerPixel!=32 || fb->depth!=24)
        FatalError("WayX currently requires one depth-24, 32-bpp screen\n");
    rrGetScrPriv(screen)->rrScreenSetSize=wayx_screen_size;
    if (rootless) rrGetScrPriv(screen)->rrCrtcSet=reject_rootless_crtc;
    RRScreenSetSizeRange(screen,1,1,16384,16384);
    if (api->window_create(backend,1,0,rootless?1:screen->width,rootless?1:screen->height)<0)
        FatalError("WayX failed to create root display\n");
    const char title[]="WayX — XLibre desktop";
    api->window_title(backend,1,(const uint8_t*)title,sizeof(title)-1);
    if (rootless) {
        if (!CompositeClientWindowType) FatalError("WayX rootless requires the Composite extension\n");
        api->window_map(backend,1,0);
        title_atom=dixAddAtom("_NET_WM_NAME");
        init_window_manager_atoms();
        if (compRedirectSubwindows(serverClient,screen->root,CompositeRedirectManual)!=Success)
            FatalError("WayX could not redirect top-level windows; Composite is required\n");
        install_window_hooks();
        register_backend();
        return;
    }
    api->window_map(backend,1,1);
    damage=DamageCreate(NULL,NULL,DamageReportNone,TRUE,screen,NULL);
    if (!damage) FatalError("WayX could not create Damage tracking\n");
    DamageRegister(&screen->GetScreenPixmap(screen)->drawable,damage);
    api->frame_damage(backend,1,0,0,screen->width,screen->height,fb->paddedBytesWidth,(const uint8_t*)fb->pfbMemory,(size_t)fb->paddedBytesWidth*screen->height);
    register_backend();
}
#ifdef GLXEXT
extern void wayx_glx_init(void);
#endif
void InitOutput(int argc,char **argv) {
#ifdef GLXEXT
    wayx_glx_init();
#endif
    if (rootless) {
        int available=api->desktop_layout(backend,&host_layout);
        if (available<0) FatalError("WayX: invalid host desktop layout\n");
        if (available) {
            vfbScreenInfoPtr fb=vfbNumScreens?&vfbScreens[0]:&defaultScreenInfo;
            if ((explicit_screen && (fb->width!=host_layout.width || fb->height!=host_layout.height || fb->depth!=24)) ||
                (explicit_crtcs && fb->numCrtcs!=(int)host_layout.count))
                FatalError("WayX rootless: -screen/-crtcs conflict with the host desktop; omit them for automatic layout\n");
            vfbCrtcInfo normalized[WAYX_MAX_MONITORS];memset(normalized,0,sizeof(normalized));
            for (unsigned i=0;i<host_layout.count;i++) {
                WayxMonitorRect m=host_layout.monitors[i];
                normalized[i]=(vfbCrtcInfo){.width=m.width,.height=m.height,.x=m.x-host_layout.origin_x,.y=m.y-host_layout.origin_y,.numOutputs=1};
                if (explicit_crtcs && (fb->crtcs[i].width!=normalized[i].width || fb->crtcs[i].height!=normalized[i].height ||
                    fb->crtcs[i].x!=normalized[i].x || fb->crtcs[i].y!=normalized[i].y))
                    FatalError("WayX rootless: -crtcs conflicts with the host monitors\n");
            }
            if (monitor_count && (monitor_count!=host_layout.count || memcmp(monitors,normalized,monitor_count*sizeof(monitors[0]))))
                FatalError("WayX rootless: -wayx-monitor conflicts with the host monitors\n");
            monitor_count=host_layout.count;memcpy(monitors,normalized,sizeof(normalized));
            desktop_origin_x=host_layout.origin_x;desktop_origin_y=host_layout.origin_y;
            host_layout_active=TRUE;
        }
    }
    if (monitor_count) {
        vfbScreenInfoPtr fb=vfbNumScreens?&vfbScreens[0]:&defaultScreenInfo;
        unsigned width=0,height=0;
        for (unsigned i=0;i<monitor_count;i++) {
            unsigned right=monitors[i].x+monitors[i].width,bottom=monitors[i].y+monitors[i].height;
            if (right>width) width=right;
            if (bottom>height) height=bottom;
        }
        if ((uint64_t)width*height*4>256u*1024u*1024u)
            FatalError("WayX monitor layout exceeds the 256 MiB framebuffer limit\n");
        fb->width=width;fb->height=height;
        vfbSetCrtcInfo(fb,monitor_count,1,1);
        memcpy(fb->crtcs,monitors,monitor_count*sizeof(monitors[0]));
    }
    WayxVfbInitOutput(argc,argv);
    if (vfbNumScreens!=1) FatalError("WayX currently supports one X screen\n");
    if (!AddCallback(&PostInitRootWindowCallback,attach_screen,NULL)) FatalError("WayX failed to register screen callback\n");
}
void ddxGiveUp(enum ExitCode error) {
    /* dix_main has already freed every Screen here. Screen-owned cleanup runs
     * from the close callback while the screen and wrapper chain are valid. */
    remove_signal_wake();
    WayxVfbGiveUp(error);
}
_X_EXPORT int wayx_xserver_run(const WayxApi *callbacks,WayxBackend *handle,int argc,char **argv) {
    if (!callbacks || callbacks->abi_version!=WAYX_ABI_VERSION || callbacks->struct_size<sizeof(WayxApi) || !handle) return 2;
    api=callbacks;backend=handle;
    /* Copy environment values: callers may free their setup storage after entry. */
    const char *base_env=getenv("WAYX_XKB_DIR"),*bin_env=getenv("WAYX_XKB_BIN_DIR");
    char *base=base_env&&*base_env?strdup(base_env):NULL;
    char *bin=bin_env&&*bin_env?strdup(bin_env):NULL;
    if((base_env&&*base_env&&!base)||(bin_env&&*bin_env&&!bin)) {free(base);free(bin);return 2;}
    const char *saved_base=XkbBaseDirectory,*saved_bin=XkbBinDirectory;
    if(base) XkbBaseDirectory=base;
    if(bin) XkbBinDirectory=bin;
    char *env[]={NULL};
    /* The embedded server shares a process with the UI and GPU workers.
     * Do not install a process-wide interval timer into this unloadable DDX. */
#ifdef HAVE_SETITIMER
    SmartScheduleSignalEnable=FALSE;
#endif
    int code=dix_main(argc,argv,env);
    XkbBaseDirectory=saved_base;XkbBinDirectory=saved_bin;
    free(base);free(bin);
    return code;
}
