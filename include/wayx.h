#ifndef WAYX_H
#define WAYX_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define WAYX_ABI_VERSION 3u
typedef struct WayxBackend WayxBackend;
enum WayxEventKind { WAYX_MOTION=1, WAYX_BUTTON, WAYX_KEY, WAYX_SCROLL,
    WAYX_FOCUS, WAYX_CLOSE, WAYX_RESIZE, WAYX_TEXT, WAYX_CLIPBOARD, WAYX_SHUTDOWN, WAYX_MOVE, WAYX_STATE };
/* RESIZE/MOVE detail may carry a nonzero sequence; Rust confirms it after output_flush. */
typedef struct WayxInputEvent {
    uint32_t kind, window, detail, flags;
    double x, y;
    uint8_t text[32];
} WayxInputEvent;
enum WayxWindowFlags { WAYX_ROOTLESS=1, WAYX_OVERRIDE_REDIRECT=2, WAYX_ABOVE=4,
    WAYX_MINIMIZED=8, WAYX_MAXIMIZED=16, WAYX_FULLSCREEN=32 };
typedef struct WayxWindowState {
    int32_t x,y;
    uint32_t flags,transient_for;
    uint32_t min_width,min_height,max_width,max_height;
    uint32_t base_width,base_height,width_inc,height_inc;
    uint32_t min_aspect_num,min_aspect_den,max_aspect_num,max_aspect_den;
} WayxWindowState;
/* All callbacks return 0 for success, negative for error, except input_poll (1/0/error).
 * Handles may be shared by threads. Buffer pointers are borrowed only during a call.
 * Frame data is little-endian XRGB8888, starting at the damage rectangle's origin.
 * Coordinates must be inside the framebuffer; stride may include padding. */
typedef struct WayxApi {
    uint32_t abi_version, struct_size;
    int32_t (*window_create)(WayxBackend*,uint32_t,uint32_t,uint32_t,uint32_t);
    int32_t (*window_destroy)(WayxBackend*,uint32_t);
    int32_t (*window_resize)(WayxBackend*,uint32_t,uint32_t,uint32_t);
    int32_t (*window_title)(WayxBackend*,uint32_t,const uint8_t*,size_t);
    int32_t (*window_map)(WayxBackend*,uint32_t,uint32_t);
    int32_t (*frame_damage)(WayxBackend*,uint32_t,uint32_t,uint32_t,uint32_t,uint32_t,size_t,const uint8_t*,size_t);
    int32_t (*input_poll)(WayxBackend*,WayxInputEvent*);
    int32_t (*clipboard_set)(WayxBackend*,const uint8_t*,size_t);
    int32_t (*clipboard_get)(WayxBackend*,uint8_t*,size_t,size_t*);
    int32_t (*window_state)(WayxBackend*,uint32_t,const WayxWindowState*);
    int32_t (*window_constrain)(WayxBackend*,uint32_t,uint32_t*,uint32_t*);
    /* Borrowed socket; register with the server wait set, never close it. */
    int32_t (*input_fd)(WayxBackend*);
    int32_t (*input_ack)(WayxBackend*);
    int32_t (*output_flush)(WayxBackend*);
} WayxApi;
const WayxApi *wayx_get_api(void);
WayxBackend *wayx_backend_new(void);
void wayx_backend_free(WayxBackend*);
int32_t wayx_window_create(WayxBackend*,uint32_t,uint32_t,uint32_t,uint32_t);
int32_t wayx_window_destroy(WayxBackend*,uint32_t);
int32_t wayx_window_resize(WayxBackend*,uint32_t,uint32_t,uint32_t);
int32_t wayx_window_title(WayxBackend*,uint32_t,const uint8_t*,size_t);
int32_t wayx_window_map(WayxBackend*,uint32_t,uint32_t);
int32_t wayx_frame_damage(WayxBackend*,uint32_t,uint32_t,uint32_t,uint32_t,uint32_t,size_t,const uint8_t*,size_t);
int32_t wayx_input_poll(WayxBackend*,WayxInputEvent*);
int32_t wayx_clipboard_set(WayxBackend*,const uint8_t*,size_t);
int32_t wayx_clipboard_get(WayxBackend*,uint8_t*,size_t,size_t*);
int32_t wayx_window_state(WayxBackend*,uint32_t,const WayxWindowState*);
int32_t wayx_window_constrain(WayxBackend*,uint32_t,uint32_t*,uint32_t*);
/* Optional XLibre DDX plugin entry, executed on a worker; native UI stays on main. */
typedef int (*WayxXserverRun)(const WayxApi*,WayxBackend*,int,char**);
#ifdef __cplusplus
}
#endif
#endif
