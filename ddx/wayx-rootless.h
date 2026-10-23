/* SPDX-License-Identifier: MIT
 * XLibre's pinned Composite implementation owns redirected window pixmaps.
 * All server objects below are touched only by the DIX worker thread. */
#include "pixmapstr.h"
#include "compositeext.h"
#include "propertyst.h"
#include "dix/property_priv.h"
#include <X11/Xatom.h>
#include <X11/extensions/composite.h>

/* Internal interfaces are tied to scripts/build-xlibre.sh's exact core revision. */
extern int compRedirectSubwindows(ClientPtr, WindowPtr, int);

typedef struct WayxWindow {
    XID id;
    unsigned int width, height;
    uint32_t move_ack,resize_ack;
    Bool geometry_pending;
    Bool seen, mapped;
    PixmapPtr pixmap;
    DamagePtr damage;
    struct WayxWindow *next;
} WayxWindow;
static WayxWindow *wayx_windows;
static Bool rootless;
static Atom title_atom;
static Bool metadata_dirty=TRUE;
static Bool window_hooks_installed;
#include "wayx-wm.h"

static void window_damage_destroyed(DamagePtr object, void *closure) {
    WayxWindow *window=closure;
    metadata_dirty=TRUE;
    window->damage=NULL;
    window->pixmap=NULL;
}
static WayxWindow *find_window(XID id) {
    for (WayxWindow *window=wayx_windows;window;window=window->next)
        if (window->id==id) return window;
    return NULL;
}
static void remove_window(WayxWindow *window) {
    if (window->damage) DamageDestroy(window->damage);
    api->window_destroy(backend,window->id);
    free(window);
}
static void window_title(WindowPtr window) {
    PropertyPtr prop=NULL;
    if (dixLookupProperty(&prop,window,title_atom,serverClient,DixReadAccess)!=Success || prop->format!=8)
        if (dixLookupProperty(&prop,window,XA_WM_NAME,serverClient,DixReadAccess)!=Success || prop->format!=8)
            return;
    if (prop->size<=4096)
        api->window_title(backend,window->drawable.id,prop->data,prop->size);
}
static void upload_window_rect(WindowPtr window, WayxWindow *tracked, BoxRec rect) {
    PixmapPtr pixmap=tracked->pixmap;
    int offset_x=window->drawable.x-pixmap->screen_x;
    int offset_y=window->drawable.y-pixmap->screen_y;
    /* Damage on a Pixmap is relative to its pixels, including the X border. */
    rect.x1-=offset_x;rect.x2-=offset_x;
    rect.y1-=offset_y;rect.y2-=offset_y;
    if (rect.x1<0) rect.x1=0;
    if (rect.y1<0) rect.y1=0;
    if (rect.x2>tracked->width) rect.x2=tracked->width;
    if (rect.y2>tracked->height) rect.y2=tracked->height;
    if (rect.x2<=rect.x1 || rect.y2<=rect.y1) return;
    int source_x=rect.x1+offset_x,source_y=rect.y1+offset_y;
    if (source_x<0 || source_y<0 || rect.x2+offset_x>pixmap->drawable.width || rect.y2+offset_y>pixmap->drawable.height)
        FatalError("WayX redirected pixmap bounds mismatch\n");
    size_t stride=pixmap->devKind;
    size_t len=(rect.y2-rect.y1-1)*stride+(rect.x2-rect.x1)*4;
    const uint8_t *pixels=(const uint8_t*)pixmap->devPrivate.ptr+source_y*stride+source_x*4;
    if (api->frame_damage(backend,tracked->id,rect.x1,rect.y1,rect.x2-rect.x1,rect.y2-rect.y1,stride,pixels,len)<0)
        FatalError("WayX rejected redirected window damage\n");
}
static void sync_rootless_windows(void) {
    if (!metadata_dirty) {
        Bool dirty=FALSE;
        for (WayxWindow *entry=wayx_windows;entry;entry=entry->next)
            if (entry->geometry_pending || (entry->mapped && (!entry->damage || !RegionNil(DamageRegion(entry->damage))))) {dirty=TRUE;break;}
        if (!dirty) return;
    }
    for (WayxWindow *entry=wayx_windows;entry;entry=entry->next) entry->seen=FALSE;
    for (WindowPtr window=screen->root->firstChild;window;window=window->nextSib) {
        if (window->drawable.class!=InputOutput) continue;
        WayxWindow *entry=find_window(window->drawable.id);
        if (!entry) {
            entry=calloc(1,sizeof(*entry));
            if (!entry) FatalError("WayX window allocation failed\n");
            entry->id=window->drawable.id;
            entry->width=window->drawable.width;entry->height=window->drawable.height;
            if (api->window_create(backend,entry->id,0,entry->width,entry->height)<0)
                FatalError("WayX rootless window limit exceeded\n");
            api->window_map(backend,entry->id,0);
            entry->next=wayx_windows;wayx_windows=entry;
        }
        entry->seen=TRUE;
        if (metadata_dirty || entry->geometry_pending) {
            window_title(window);
            sync_window_state(window,entry);
            entry->geometry_pending=FALSE;
        }
        Bool mapped=window->mapped && window->realized;
        if (entry->mapped!=mapped) {
            api->window_map(backend,entry->id,mapped);
            entry->mapped=mapped;
        }
        if (!mapped) continue;
        Bool full=FALSE;
        if (entry->width!=window->drawable.width || entry->height!=window->drawable.height) {
            entry->width=window->drawable.width;entry->height=window->drawable.height;
            if (api->window_resize(backend,entry->id,entry->width,entry->height)<0)
                FatalError("WayX rootless resize rejected\n");
            full=TRUE;
        }
        PixmapPtr pixmap=screen->GetWindowPixmap(window);
        if (!pixmap || !pixmap->devPrivate.ptr || pixmap->drawable.bitsPerPixel!=32)
            FatalError("WayX rootless requires a software 32-bpp pixmap\n");
        if (pixmap!=entry->pixmap || !entry->damage) {
            if (entry->damage) DamageDestroy(entry->damage);
            entry->pixmap=pixmap;
            entry->damage=DamageCreate(NULL,window_damage_destroyed,DamageReportNone,TRUE,screen,entry);
            if (!entry->damage) FatalError("WayX window Damage allocation failed\n");
            DamageRegister(&pixmap->drawable,entry->damage);
            full=TRUE;
        }
        if (full) {
            int x=window->drawable.x-pixmap->screen_x,y=window->drawable.y-pixmap->screen_y;
            BoxRec rect={x,y,x+entry->width,y+entry->height};
            upload_window_rect(window,entry,rect);
        } else {
            RegionPtr region=DamageRegion(entry->damage);
            BoxPtr boxes=RegionRects(region);
            for(int i=0;i<RegionNumRects(region);i++) upload_window_rect(window,entry,boxes[i]);
        }
        DamageEmpty(entry->damage);
    }
    metadata_dirty=FALSE;
    WayxWindow **link=&wayx_windows;
    while (*link) {
        WayxWindow *entry=*link;
        if (!entry->seen) {*link=entry->next;remove_window(entry);}
        else link=&entry->next;
    }
}

/* Mirror XWin's screen-hook approach: metadata is inspected after mutations,
 * never re-read on every input event or clipboard maintenance request. Hooks
 * are unwrapped around calls so Composite and other wrappers remain chained. */
/* Xwindows.h renames the Screen member while scrnintstr.h is parsed, then
 * another header can undefine CreateWindow. Do not depend on its later macro state. */
#ifndef WAYX_CREATE_WINDOW_MEMBER
#ifdef WIN32
#define WAYX_CREATE_WINDOW_MEMBER CreateWindowA
#else
#define WAYX_CREATE_WINDOW_MEMBER CreateWindow
#endif
#endif
#define WAYX_BOOL_HOOK(name,member,params,args) \
static name##ProcPtr saved_##name; \
static Bool wayx_##name params { \
    ScreenPtr s=window->drawable.pScreen; metadata_dirty=TRUE; \
    s->member=saved_##name; Bool result=s->member?s->member args:TRUE; \
    saved_##name=s->member;s->member=wayx_##name;return result; }
#define WAYX_VOID_HOOK(name,member,params,args) \
static name##ProcPtr saved_##name; \
static void wayx_##name params { \
    ScreenPtr s=window->drawable.pScreen; metadata_dirty=TRUE; \
    s->member=saved_##name;if(s->member) s->member args; \
    saved_##name=s->member;s->member=wayx_##name; }
WAYX_BOOL_HOOK(CreateWindow,WAYX_CREATE_WINDOW_MEMBER,(WindowPtr window),(window))
WAYX_BOOL_HOOK(DestroyWindow,DestroyWindow,(WindowPtr window),(window))
WAYX_BOOL_HOOK(PositionWindow,PositionWindow,(WindowPtr window,int x,int y),(window,x,y))
WAYX_BOOL_HOOK(RealizeWindow,RealizeWindow,(WindowPtr window),(window))
WAYX_BOOL_HOOK(UnrealizeWindow,UnrealizeWindow,(WindowPtr window),(window))
WAYX_BOOL_HOOK(ChangeWindowAttributes,ChangeWindowAttributes,(WindowPtr window,unsigned long mask),(window,mask))
WAYX_VOID_HOOK(ReparentWindow,ReparentWindow,(WindowPtr window,WindowPtr parent),(window,parent))
WAYX_VOID_HOOK(RestackWindow,RestackWindow,(WindowPtr window,WindowPtr sibling),(window,sibling))
WAYX_VOID_HOOK(SetWindowPixmap,SetWindowPixmap,(WindowPtr window,PixmapPtr pixmap),(window,pixmap))
WAYX_VOID_HOOK(ResizeWindow,ResizeWindow,(WindowPtr window,int x,int y,unsigned int width,unsigned int height,WindowPtr sibling),(window,x,y,width,height,sibling))
WAYX_VOID_HOOK(ChangeBorderWidth,ChangeBorderWidth,(WindowPtr window,unsigned int width),(window,width))
#undef WAYX_BOOL_HOOK
#undef WAYX_VOID_HOOK
static void metadata_property(CallbackListPtr *list,void *closure,void *data) {
    PropertyFilterParam *property=data;
    if ((property->access_mode & (DixWriteAccess|DixDestroyAccess|DixRemoveAccess)) || property->delete) metadata_dirty=TRUE;
}
#define WAYX_SCREEN_HOOKS(X) \
    X(CreateWindow,WAYX_CREATE_WINDOW_MEMBER) X(DestroyWindow,DestroyWindow) X(PositionWindow,PositionWindow) X(RealizeWindow,RealizeWindow) \
    X(UnrealizeWindow,UnrealizeWindow) X(ChangeWindowAttributes,ChangeWindowAttributes) X(ReparentWindow,ReparentWindow) \
    X(RestackWindow,RestackWindow) X(SetWindowPixmap,SetWindowPixmap) \
    X(ResizeWindow,ResizeWindow) X(ChangeBorderWidth,ChangeBorderWidth)
static void install_window_hooks(void) {
    window_hooks_installed=TRUE;
#define INSTALL(name,member) saved_##name=screen->member;screen->member=wayx_##name;
    WAYX_SCREEN_HOOKS(INSTALL)
#undef INSTALL
    if(!AddCallback(&PropertyFilterCallback,metadata_property,NULL))
        FatalError("WayX could not register property change tracking\n");
}
static void remove_window_hooks(void) {
    if(!window_hooks_installed) return;
    window_hooks_installed=FALSE;
    DeleteCallback(&PropertyFilterCallback,metadata_property,NULL);
#define REMOVE(name,member) screen->member=saved_##name;
    WAYX_SCREEN_HOOKS(REMOVE)
#undef REMOVE
}
#undef WAYX_SCREEN_HOOKS
