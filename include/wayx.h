#ifndef WAYX_H
#define WAYX_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define WAYX_ABI_VERSION 1u
typedef struct WayxBackend WayxBackend;
enum WayxEventKind { WAYX_MOTION=1, WAYX_BUTTON, WAYX_KEY, WAYX_SCROLL,
    WAYX_FOCUS, WAYX_CLOSE, WAYX_RESIZE, WAYX_TEXT, WAYX_CLIPBOARD, WAYX_SHUTDOWN };
typedef struct WayxInputEvent {
    uint32_t kind, window, detail, flags;
    double x, y;
    uint8_t text[32];
} WayxInputEvent;
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
/* Optional XLibre DDX plugin entry, executed on a worker; native UI stays on main. */
typedef int (*WayxXserverRun)(const WayxApi*,WayxBackend*,int,char**);
#ifdef __cplusplus
}
#endif
#endif
