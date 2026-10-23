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
    unsigned dirty;
    Bool queued;
    WindowPtr window;
    WayxWindowState state;
    struct WayxWindow *hash_next,*dirty_next;
    Bool seen, mapped;
    PixmapPtr pixmap;
    DamagePtr damage;
    struct WayxWindow *next;
} WayxWindow;
static WayxWindow *wayx_windows;
static Bool rootless;
static Atom title_atom;
enum { WINDOW_GEOMETRY=1, WINDOW_HINTS=2, WINDOW_TITLE=4, WINDOW_PIXELS=8, WINDOW_ALL=15 };
static Bool lifecycle_dirty=TRUE;
static WayxWindow *window_index[256],*dirty_windows;
static void dirty_window(WayxWindow *entry,unsigned flags) {
    if (!entry || !flags) return;
    entry->dirty|=flags;
    if (!entry->queued) {entry->dirty_next=dirty_windows;dirty_windows=entry;entry->queued=TRUE;}
}
static WayxWindow *find_window(XID id) {
    for (WayxWindow *entry=window_index[id%256];entry;entry=entry->hash_next)
        if (entry->id==id) return entry;
    return NULL;
}
static void dirty_top(WindowPtr window,unsigned flags) {
    while (window && window->parent && window->parent!=screen->root) window=window->parent;
    if (window) dirty_window(find_window(window->drawable.id),flags);
}
static void dirty_all_geometry(void) {
    for (WayxWindow *entry=wayx_windows;entry;entry=entry->next) dirty_window(entry,WINDOW_GEOMETRY);
}
static Bool window_hooks_installed;
#include "wayx-wm.h"

static void window_damage_destroyed(DamagePtr object, void *closure) {
    WayxWindow *window=closure;
    dirty_window(window,WINDOW_PIXELS);
    window->damage=NULL;
    window->pixmap=NULL;
}
static void window_damaged(DamagePtr object,RegionPtr region,void *closure) {
    dirty_window(closure,WINDOW_PIXELS);
}
static void remove_window(WayxWindow *window) {
    if (window->damage) DamageDestroy(window->damage);
    WayxWindow **link=&window_index[window->id%256];
    while (*link && *link!=window) link=&(*link)->hash_next;
    if (*link) *link=window->hash_next;
    link=&dirty_windows;
    while (*link && *link!=window) link=&(*link)->dirty_next;
    if (*link) *link=window->dirty_next;
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
    if (lifecycle_dirty) {
        lifecycle_dirty=FALSE;
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
                entry->hash_next=window_index[entry->id%256];window_index[entry->id%256]=entry;
                dirty_window(entry,WINDOW_ALL);
            }
            entry->seen=TRUE;entry->window=window;
            if (entry->mapped!=(window->mapped && window->realized)) dirty_window(entry,WINDOW_ALL);
        }
        WayxWindow **link=&wayx_windows;
        while (*link) {
            WayxWindow *entry=*link;
            if (!entry->seen) {*link=entry->next;remove_window(entry);}
            else link=&entry->next;
        }
    }
    while (dirty_windows) {
        WayxWindow *entry=dirty_windows;
        dirty_windows=entry->dirty_next;entry->queued=FALSE;entry->dirty_next=NULL;
        unsigned flags=entry->dirty;entry->dirty=0;
        WindowPtr window=entry->window;
        if (flags&WINDOW_TITLE) window_title(window);
        if ((flags&(WINDOW_GEOMETRY|WINDOW_HINTS)) || entry->geometry_pending) {
            sync_window_state(window,entry,flags&WINDOW_HINTS);
            entry->geometry_pending=FALSE;
        }
        Bool mapped=window->mapped && window->realized;
        if (entry->mapped!=mapped) {
            api->window_map(backend,entry->id,mapped);
            entry->mapped=mapped;
        }
        if (!mapped || !(flags&WINDOW_PIXELS)) continue;
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
            entry->damage=DamageCreate(window_damaged,window_damage_destroyed,DamageReportNonEmpty,TRUE,screen,entry);
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
#define WAYX_BOOL_HOOK(name,member,flags,life,params,args) \
static name##ProcPtr saved_##name; \
static Bool wayx_##name params { \
    ScreenPtr s=window->drawable.pScreen; dirty_top(window,flags); if(life) lifecycle_dirty=TRUE; \
    s->member=saved_##name; Bool result=s->member?s->member args:TRUE; \
    saved_##name=s->member;s->member=wayx_##name;return result; }
#define WAYX_VOID_HOOK(name,member,flags,life,params,args) \
static name##ProcPtr saved_##name; \
static void wayx_##name params { \
    ScreenPtr s=window->drawable.pScreen; dirty_top(window,flags); if(life) lifecycle_dirty=TRUE; \
    s->member=saved_##name;if(s->member) s->member args; \
    saved_##name=s->member;s->member=wayx_##name; }
WAYX_BOOL_HOOK(CreateWindow,WAYX_CREATE_WINDOW_MEMBER,WINDOW_ALL,TRUE,(WindowPtr window),(window))
WAYX_BOOL_HOOK(DestroyWindow,DestroyWindow,WINDOW_GEOMETRY,TRUE,(WindowPtr window),(window))
WAYX_BOOL_HOOK(PositionWindow,PositionWindow,WINDOW_GEOMETRY|WINDOW_PIXELS,FALSE,(WindowPtr window,int x,int y),(window,x,y))
WAYX_BOOL_HOOK(RealizeWindow,RealizeWindow,WINDOW_ALL,TRUE,(WindowPtr window),(window))
WAYX_BOOL_HOOK(UnrealizeWindow,UnrealizeWindow,WINDOW_ALL,TRUE,(WindowPtr window),(window))
WAYX_BOOL_HOOK(ChangeWindowAttributes,ChangeWindowAttributes,WINDOW_HINTS|WINDOW_GEOMETRY,FALSE,(WindowPtr window,unsigned long mask),(window,mask))
WAYX_VOID_HOOK(ReparentWindow,ReparentWindow,WINDOW_ALL,TRUE,(WindowPtr window,WindowPtr parent),(window,parent))
WAYX_VOID_HOOK(RestackWindow,RestackWindow,WINDOW_GEOMETRY,FALSE,(WindowPtr window,WindowPtr sibling),(window,sibling))
WAYX_VOID_HOOK(SetWindowPixmap,SetWindowPixmap,WINDOW_PIXELS,FALSE,(WindowPtr window,PixmapPtr pixmap),(window,pixmap))
WAYX_VOID_HOOK(ResizeWindow,ResizeWindow,WINDOW_GEOMETRY|WINDOW_PIXELS,FALSE,(WindowPtr window,int x,int y,unsigned int width,unsigned int height,WindowPtr sibling),(window,x,y,width,height,sibling))
WAYX_VOID_HOOK(ChangeBorderWidth,ChangeBorderWidth,WINDOW_GEOMETRY|WINDOW_PIXELS,FALSE,(WindowPtr window,unsigned int width),(window,width))
#undef WAYX_BOOL_HOOK
#undef WAYX_VOID_HOOK
static unsigned property_dirty(Atom atom) {
    if (atom==title_atom || atom==XA_WM_NAME) return WINDOW_TITLE;
    if (atom==XA_WM_NORMAL_HINTS || atom==XA_WM_TRANSIENT_FOR || atom==net_state)
        return WINDOW_HINTS|WINDOW_GEOMETRY;
    return 0;
}
static void metadata_property(CallbackListPtr *list,void *closure,void *data) {
    PropertyFilterParam *property=data;
    if (!((property->access_mode & (DixWriteAccess|DixDestroyAccess|DixRemoveAccess)) || property->delete)) return;
    unsigned flags=property_dirty(property->property);
    for (size_t i=0;i<property->nAtoms;i++) flags|=property_dirty(property->atoms[i]);
    /* Only top-level properties form native metadata. Child properties do not. */
    dirty_window(find_window(property->window),flags);
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
