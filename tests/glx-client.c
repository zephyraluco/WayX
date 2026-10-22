/* SPDX-License-Identifier: MIT
 * Real Mesa direct software GLX over TCP: VBO draw, readback, then X11 present. */
#define GL_GLEXT_PROTOTYPES
#include <GL/gl.h>
#include <GL/glext.h>
#include <GL/glx.h>
#include <X11/Xlib.h>
#include <stdio.h>
#include <stdlib.h>
int main(void) {
    setbuf(stdout,NULL);
    Display *d=XOpenDisplay(NULL);
    if(!d) return 2;
    /* Older Mesa compares even pbuffer limits when matching window configs.
     * Require a window-only config (limits omitted from the wire), alongside
     * the actual context, drawing and presentation checks below. */
    int available=0,legacy_window=0;
    GLXFBConfig *all=glXGetFBConfigs(d,DefaultScreen(d),&available);
    for(int i=0;all && i<available;i++) {
        int type,depth,stencil,db;
        if(glXGetFBConfigAttrib(d,all[i],GLX_DRAWABLE_TYPE,&type) ||
           glXGetFBConfigAttrib(d,all[i],GLX_DEPTH_SIZE,&depth) ||
           glXGetFBConfigAttrib(d,all[i],GLX_STENCIL_SIZE,&stencil) ||
           glXGetFBConfigAttrib(d,all[i],GLX_DOUBLEBUFFER,&db)) return 11;
        if((type&GLX_WINDOW_BIT) && !(type&GLX_PBUFFER_BIT) &&
           depth==24 && stencil==8 && db)
            legacy_window=1;
    }
    XFree(all);
    if(!legacy_window) {fputs("No legacy Mesa window config\n",stderr);return 12;}
    int attrs[]={GLX_RGBA,GLX_DOUBLEBUFFER,GLX_DEPTH_SIZE,24,GLX_STENCIL_SIZE,8,None};
    XVisualInfo *visual=glXChooseVisual(d,DefaultScreen(d),attrs);
    if(!visual) {fputs("No compatible GLX visual\n",stderr);return 3;}
    XSetWindowAttributes wa={0};
    wa.colormap=XCreateColormap(d,DefaultRootWindow(d),visual->visual,AllocNone);
    Window w=XCreateWindow(d,DefaultRootWindow(d),20,20,64,64,0,visual->depth,InputOutput,visual->visual,CWColormap,&wa);
    XStoreName(d,w,"WayX GLX validation");
    XMapWindow(d,w);XSync(d,False);
    int fbattrs[]={GLX_X_RENDERABLE,True,GLX_DRAWABLE_TYPE,GLX_WINDOW_BIT,
        GLX_RENDER_TYPE,GLX_RGBA_BIT,GLX_DOUBLEBUFFER,True,GLX_DEPTH_SIZE,24,GLX_STENCIL_SIZE,8,None};
    int count=0;GLXFBConfig *configs=glXChooseFBConfig(d,DefaultScreen(d),fbattrs,&count);
    PFNGLXCREATECONTEXTATTRIBSARBPROC create=(PFNGLXCREATECONTEXTATTRIBSARBPROC)glXGetProcAddressARB((const GLubyte*)"glXCreateContextAttribsARB");
    if(!create || !configs || !count) return 10;
    int context_attrs[]={GLX_CONTEXT_MAJOR_VERSION_ARB,2,GLX_CONTEXT_MINOR_VERSION_ARB,1,None};
    GLXContext context=create(d,configs[0],NULL,True,context_attrs);
    XFree(configs);
    if(!context || !glXMakeCurrent(d,w,context)) {fputs("GLX context unavailable\n",stderr);return 4;}
    const char *version=(const char*)glGetString(GL_VERSION);
    if(!glXIsDirect(d,context) || !version || atof(version)<2.1) return 5;
    PFNGLGENBUFFERSPROC gen=(PFNGLGENBUFFERSPROC)glXGetProcAddressARB((const GLubyte*)"glGenBuffers");
    PFNGLBINDBUFFERPROC bind=(PFNGLBINDBUFFERPROC)glXGetProcAddressARB((const GLubyte*)"glBindBuffer");
    PFNGLBUFFERDATAPROC data=(PFNGLBUFFERDATAPROC)glXGetProcAddressARB((const GLubyte*)"glBufferData");
    PFNGLDELETEBUFFERSPROC del=(PFNGLDELETEBUFFERSPROC)glXGetProcAddressARB((const GLubyte*)"glDeleteBuffers");
    if(!gen || !bind || !data || !del) return 6;
    GLuint buffer;const GLfloat points[]={-.8f,-.8f,.8f,-.8f,0,.8f};
    gen(1,&buffer);bind(GL_ARRAY_BUFFER,buffer);data(GL_ARRAY_BUFFER,sizeof(points),points,GL_STATIC_DRAW);
    glViewport(0,0,64,64);glClearColor(0,0,1,1);glClear(GL_COLOR_BUFFER_BIT|GL_DEPTH_BUFFER_BIT);
    glEnableClientState(GL_VERTEX_ARRAY);glVertexPointer(2,GL_FLOAT,0,NULL);glColor3f(1,0,0);glDrawArrays(GL_TRIANGLES,0,3);
    unsigned char pixel[4];glReadPixels(32,32,1,1,GL_RGBA,GL_UNSIGNED_BYTE,pixel);
    if(glGetError()!=GL_NO_ERROR || pixel[0]<250 || pixel[1]>5 || pixel[2]>5) return 7;
    glReadPixels(2,2,1,1,GL_RGBA,GL_UNSIGNED_BYTE,pixel);
    if(pixel[0]>5 || pixel[1]>5 || pixel[2]<250) return 9;
    glXSwapBuffers(d,w);XSync(d,False);
    printf("GLX_READY %lu %s / %s\n",w,version,glGetString(GL_RENDERER));
    (void)getchar();
    del(1,&buffer);glXMakeCurrent(d,None,NULL);glXDestroyContext(d,context);
    XDestroyWindow(d,w);XFreeColormap(d,wa.colormap);XFree(visual);XCloseDisplay(d);
    return 0;
}
