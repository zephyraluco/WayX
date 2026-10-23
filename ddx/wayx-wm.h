/* SPDX-License-Identifier: MIT
 * ICCCM metadata conversion and native input -> X11 window management. */
#include "property.h"
/* ICCCM WM_STATE values; no client-side Xlib headers in the DDX. */
enum { WAYX_WM_NORMAL_STATE=1, WAYX_WM_ICONIC_STATE=3 };
static Atom wm_protocols,wm_delete,wm_state,net_state,net_above,net_hidden;
static Atom net_max_horz,net_max_vert,net_fullscreen;
static PropertyPtr property(WindowPtr window,Atom atom,int format) {
    PropertyPtr value=NULL;
    if(dixLookupProperty(&value,window,atom,serverClient,DixReadAccess)!=Success || value->format!=format) return NULL;
    return value;
}
static uint32_t hint_dimension(CARD32 value) {return value>16384?16384:value;}
static void sync_window_state(WindowPtr window,WayxWindow *tracked,Bool hints) {
    WayxWindowState state=tracked->state;
    if (hints) {
        memset(&state,0,sizeof(state));
        state.flags=WAYX_ROOTLESS|(window->overrideRedirect?WAYX_OVERRIDE_REDIRECT:0);
        PropertyPtr value=property(window,XA_WM_TRANSIENT_FOR,32);
        if(value && value->size>=1) state.transient_for=((CARD32*)value->data)[0];
        value=property(window,XA_WM_NORMAL_HINTS,32);
        if(value && value->size>=15) {
            CARD32 *h=value->data;CARD32 flags=h[0];
            if(flags&(1<<4)) {state.min_width=hint_dimension(h[5]);state.min_height=hint_dimension(h[6]);}
            if(flags&(1<<5)) {state.max_width=hint_dimension(h[7]);state.max_height=hint_dimension(h[8]);}
            state.base_width=state.min_width;state.base_height=state.min_height;
            if(flags&(1<<6)) {state.width_inc=hint_dimension(h[9]);state.height_inc=hint_dimension(h[10]);}
            if(flags&(1<<7)) {
                state.min_aspect_num=h[11];state.min_aspect_den=h[12];
                state.max_aspect_num=h[13];state.max_aspect_den=h[14];
            }
            if((flags&(1<<8)) && value->size>=17) {state.base_width=hint_dimension(h[15]);state.base_height=hint_dimension(h[16]);}
        }
        value=property(window,net_state,32);
        Bool max_h=FALSE,max_v=FALSE;
        if(value) for(uint32_t i=0;i<value->size;i++) {
            Atom atom=((CARD32*)value->data)[i];
            if(atom==net_above) state.flags|=WAYX_ABOVE;
            if(atom==net_hidden) state.flags|=WAYX_MINIMIZED;
            if(atom==net_fullscreen) state.flags|=WAYX_FULLSCREEN;
            if(atom==net_max_horz) max_h=TRUE;
            if(atom==net_max_vert) max_v=TRUE;
        }
        if(max_h&&max_v) state.flags|=WAYX_MAXIMIZED;
    }
    int64_t host_x=(int64_t)window->drawable.x-window->borderWidth+desktop_origin_x;
    int64_t host_y=(int64_t)window->drawable.y-window->borderWidth+desktop_origin_y;
    state.x=host_x<INT32_MIN?INT32_MIN:host_x>INT32_MAX?INT32_MAX:host_x;
    state.y=host_y<INT32_MIN?INT32_MIN:host_y>INT32_MAX?INT32_MAX:host_y;
    tracked->state=state;
    WayxGeometrySnapshot geometry={0};
    geometry.state=state;
    geometry.width=window->drawable.width;geometry.height=window->drawable.height;
    geometry.move_ack=tracked->move_ack;geometry.resize_ack=tracked->resize_ack;
    if(api->window_geometry(backend,window->drawable.id,&geometry)<0) {
        /* Malformed client hints must not terminate the display server. */
        WayxWindowState fallback={0};fallback.x=state.x;fallback.y=state.y;
        fallback.flags=state.flags;fallback.transient_for=state.transient_for==window->drawable.id?0:state.transient_for;
        geometry.state=fallback;tracked->state=fallback;
        if(api->window_geometry(backend,window->drawable.id,&geometry)<0)
            FatalError("WayX geometry snapshot rejected\n");
    }
}
static void close_window(WindowPtr window) {
    PropertyPtr protocols=property(window,wm_protocols,32);
    if(protocols) for(uint32_t i=0;i<protocols->size;i++) if(((CARD32*)protocols->data)[i]==wm_delete) {
        xEvent event={0};event.u.u.type=ClientMessage|0x80;event.u.u.detail=32;
        event.u.clientMessage.window=window->drawable.id;
        event.u.clientMessage.u.l.type=wm_protocols;
        event.u.clientMessage.u.l.longs0=wm_delete;
        event.u.clientMessage.u.l.longs1=GetTimeInMillis();
        DeliverEventsToWindow(GetMaster(wayx_keyboard,MASTER_KEYBOARD),window,&event,1,NoEventMask,NULL);
        return;
    }
    ClientPtr client=dixClientForXID(window->drawable.id);
    if(client && client!=serverClient) CloseDownClient(client);
}
static void native_window_state(WindowPtr window,uint32_t flags) {
    dirty_top(window,WINDOW_HINTS|WINDOW_GEOMETRY);
    CARD32 atoms[6];unsigned int count=0;
    /* Preserve client states that this backend does not own. */
    PropertyPtr old=property(window,net_state,32);
    Atom extras[64];unsigned int extra_count=0;
    if(old) for(uint32_t i=0;i<old->size && extra_count<64;i++) {
        Atom atom=((CARD32*)old->data)[i];
        if(atom!=net_hidden&&atom!=net_max_horz&&atom!=net_max_vert&&atom!=net_fullscreen) extras[extra_count++]=atom;
    }
    if(flags&WAYX_MINIMIZED) atoms[count++]=net_hidden;
    if(flags&WAYX_MAXIMIZED) {atoms[count++]=net_max_horz;atoms[count++]=net_max_vert;}
    if(flags&WAYX_FULLSCREEN) atoms[count++]=net_fullscreen;
    CARD32 combined[70];unsigned int n=0;
    for(unsigned int i=0;i<extra_count;i++) combined[n++]=extras[i];
    for(unsigned int i=0;i<count;i++) combined[n++]=atoms[i];
    dixChangeWindowProperty(serverClient,window,net_state,XA_ATOM,32,PropModeReplace,n,combined,TRUE);
    CARD32 state[]={flags&WAYX_MINIMIZED?WAYX_WM_ICONIC_STATE:WAYX_WM_NORMAL_STATE,0};
    dixChangeWindowProperty(serverClient,window,wm_state,wm_state,32,PropModeReplace,2,state,TRUE);
}
static void init_window_manager_atoms(void) {
    wm_protocols=dixAddAtom("WM_PROTOCOLS");wm_delete=dixAddAtom("WM_DELETE_WINDOW");
    wm_state=dixAddAtom("WM_STATE");net_state=dixAddAtom("_NET_WM_STATE");
    net_above=dixAddAtom("_NET_WM_STATE_ABOVE");net_hidden=dixAddAtom("_NET_WM_STATE_HIDDEN");
    net_max_horz=dixAddAtom("_NET_WM_STATE_MAXIMIZED_HORZ");net_max_vert=dixAddAtom("_NET_WM_STATE_MAXIMIZED_VERT");
    net_fullscreen=dixAddAtom("_NET_WM_STATE_FULLSCREEN");
}
