#include "wayx.h"
#include <assert.h>
#include <string.h>
int main(void) {
    const WayxApi *api=wayx_get_api();
    assert(api->abi_version==WAYX_ABI_VERSION);
    assert(api->struct_size==sizeof(*api));
    assert(sizeof(WayxInputEvent)==64);
    assert(sizeof(WayxWindowState)==64);
    assert(sizeof(WayxGeometrySnapshot)==80);
    WayxBackend *b=wayx_backend_new();
    assert(api->window_create(b,1,0,4,3)==0);
    assert(api->window_create(b,1,0,4,3)<0);
    WayxWindowState state={0};state.min_width=20;state.min_height=10;state.width_inc=8;
    assert(api->window_state(b,1,&state)==0);
    WayxGeometrySnapshot geometry={0};geometry.state=state;
    geometry.width=4;geometry.height=3;geometry.move_ack=11;geometry.resize_ack=17;
    assert(api->window_geometry(b,1,&geometry)==0);
    assert(api->output_flush(b)==0);
    uint32_t width=31,height=3;
    assert(api->window_constrain(b,1,&width,&height)==0 && width==24 && height==10);
    assert(api->window_constrain(b,1,&width,&width)<0);
    state.max_width=1;
    assert(api->window_state(b,1,&state)<0);
    uint8_t pixels[16]={0,0,255,0, 0,255,0,0, 255,0,0,0, 255,255,255,0};
    assert(api->frame_damage(b,1,1,1,2,2,8,pixels,sizeof pixels)==0);
    assert(api->frame_damage(b,1,3,2,2,2,8,pixels,sizeof pixels)<0);
    assert(api->clipboard_set(b,(const uint8_t*)"hello",5)==0);
    size_t n=0;
    assert(api->clipboard_get(b,NULL,0,&n)==0 && n==5);
    uint8_t text[5];
    assert(api->clipboard_get(b,text,sizeof text,&n)==0);
    assert(memcmp(text,"hello",5)==0);
    WayxInputEvent event;
    assert(api->input_poll(b,&event)==0);
    assert(api->window_destroy(b,1)==0);
    wayx_backend_free(b);
    return 0;
}
