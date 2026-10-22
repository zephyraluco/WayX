/* SPDX-License-Identifier: MIT
 * GLX drawable/FBConfig negotiation for Mesa's client-side drisw renderer.
 * No server-side OpenGL context is exposed: rendering arrives as X11 images.
 */
#include <dix-config.h>
#ifdef GLXEXT
#include <GL/glxtokens.h>
#include "Xext/glx/glxserver.h"
#include "Xext/glx/glxutil.h"
#include "Xext/glx/extension_string.h"
#include "glx_extinit.h"

static __GLXcontext *reject_indirect(__GLXscreen *screen, __GLXconfig *config,
    __GLXcontext *share, unsigned count, const uint32_t *attributes, int *error) {
    *error=BadMatch;
    return NULL;
}
static void destroy_drawable(__GLXdrawable *drawable) {
    __glXDrawableRelease(drawable);
    free(drawable);
}
static GLboolean swap_drawable(ClientPtr client, __GLXdrawable *drawable) {
    /* The client renderer's PutImage has already presented the back buffer. */
    return TRUE;
}
static __GLXdrawable *create_drawable(ClientPtr client, __GLXscreen *screen,
    DrawablePtr draw, XID id, int type, XID glx_id, __GLXconfig *config) {
    __GLXdrawable *drawable=calloc(1,sizeof(*drawable));
    if (!drawable) return NULL;
    if (!__glXDrawableInit(drawable,screen,draw,type,glx_id,config)) {
        free(drawable);return NULL;
    }
    drawable->destroy=destroy_drawable;
    drawable->swapBuffers=swap_drawable;
    return drawable;
}
static void destroy_screen(__GLXscreen *screen) {
    __glXScreenDestroy(screen);
    free(screen);
}
static __GLXscreen *probe_screen(ScreenPtr screen) {
    __GLXscreen *glx=calloc(1,sizeof(*glx));
    if (!glx) return NULL;
    glx->destroy=destroy_screen;
    glx->createContext=reject_indirect;
    glx->createDrawable=create_drawable;
    /* Standard RGBA formats used by Mesa drisw. Depth/stencil are allocated
     * by the remote renderer; only the 24-bit XRGB color image crosses X11. */
    __GLXconfig **next=&glx->fbconfigs;
    for (int double_buffer=0;double_buffer<2;double_buffer++) {
        for (int depth=0;depth<2;depth++) {
            __GLXconfig *config=calloc(1,sizeof(*config));
            if (!config) {destroy_screen(glx);return NULL;}
            config->visualType=GLX_TRUE_COLOR;
            config->redBits=config->greenBits=config->blueBits=8;
            config->redMask=0xff0000;config->greenMask=0xff00;config->blueMask=0xff;
            config->rgbBits=24;
            config->bindToTextureRgb=config->bindToTextureRgba=config->yInverted=TRUE;
            config->bindToTextureTargets=GLX_TEXTURE_1D_BIT_EXT|GLX_TEXTURE_2D_BIT_EXT|GLX_TEXTURE_RECTANGLE_BIT_EXT;
            config->depthBits=depth?24:0;
            config->stencilBits=depth?8:0;
            config->doubleBufferMode=double_buffer;
            config->drawableType=GLX_WINDOW_BIT|GLX_PIXMAP_BIT|GLX_PBUFFER_BIT;
            config->renderType=GLX_RGBA_BIT;
            config->visualRating=GLX_NONE;
            config->transparentPixel=GLX_NONE;
            config->maxPbufferWidth=config->maxPbufferHeight=16384;
            config->maxPbufferPixels=16384*16384;
            config->swapMethod=GLX_SWAP_COPY_OML;
            *next=config;next=&config->next;
        }
    }
    __glXInitExtensionEnableBits(glx->glx_enable_bits);
    __glXEnableExtension(glx->glx_enable_bits,"GLX_ARB_create_context");
    __glXEnableExtension(glx->glx_enable_bits,"GLX_ARB_create_context_profile");
    glx->glvnd=strdup("mesa");
    __glXScreenInit(glx,screen);
    LogMessage(X_INFO,"WayX: client-side Mesa GLX enabled; indirect rendering unavailable\n");
    return glx;
}
static glx_func_ptr no_indirect_proc(const char *name) { return NULL; }
static __GLXprovider provider={probe_screen,"WayX client Mesa",NULL};
void wayx_glx_init(void);
void wayx_glx_init(void) { __glXsetGetProcAddress(no_indirect_proc); GlxPushProvider(&provider); }
#endif
