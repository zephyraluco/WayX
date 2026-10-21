/* SPDX-License-Identifier: MIT
 * Observe actual host windows, not only backend records or open sockets. */
#include <X11/Xlib.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static int handle_xerror(Display *display,XErrorEvent *error) {
    (void)display;
    /* QueryTree and GetProperty are separate requests: a native window may
     * disappear between them during the close-lifecycle assertion. */
    if (error->error_code==BadWindow) return 0;
    fprintf(stderr,"Unexpected X11 error %u in request %u\n",error->error_code,error->request_code);
    exit(1);
}
static void pause_poll(void) { struct timespec delay={0,10000000};nanosleep(&delay,NULL); }
static Window named_window(Display *display,const char *expected) {
    Window root,parent,*children=NULL,found=0;unsigned int count=0;
    if (!XQueryTree(display,DefaultRootWindow(display),&root,&parent,&children,&count)) return 0;
    for(unsigned int i=0;i<count;i++) {
        char *name=NULL;
        if(XFetchName(display,children[i],&name) && name) {
            if(!strcmp(name,expected)) found=children[i];
            if(!strcmp(name,"WayX — XLibre desktop")) {fprintf(stderr,"Root desktop leaked into rootless mode\n");exit(1);}
            XFree(name);
        }
    }
    if(children) XFree(children);
    return found;
}
static void close_native(Display *display,Window window) {
    XEvent event={0};event.xclient.type=ClientMessage;event.xclient.window=window;
    event.xclient.message_type=XInternAtom(display,"WM_PROTOCOLS",False);event.xclient.format=32;
    event.xclient.data.l[0]=XInternAtom(display,"WM_DELETE_WINDOW",False);
    XSendEvent(display,window,False,NoEventMask,&event);XFlush(display);
}
int main(int argc,char **argv) {
    if(argc!=3) return 2;
    XSetErrorHandler(handle_xerror);
    Display *remote=NULL;
    for(int i=0;i<500 && !remote;i++) {remote=XOpenDisplay(argv[1]);if(!remote) pause_poll();}
    Display *host=XOpenDisplay(argv[2]);
    if(!remote || !host) {fprintf(stderr,"Could not connect to test displays\n");return 1;}
    Window root=DefaultRootWindow(remote);
    Window a=XCreateSimpleWindow(remote,root,10,20,100,80,0,0,0xff0000);
    Window b=XCreateSimpleWindow(remote,root,40,50,100,80,0,0,0x0000ff);
    XStoreName(remote,a,"WayX native A");XStoreName(remote,b,"WayX native B");
    XMapWindow(remote,a);XMapWindow(remote,b);XFlush(remote);
    Window native_a=0,native_b=0;
    for(int i=0;i<500;i++) {
        native_a=named_window(host,"WayX native A");native_b=named_window(host,"WayX native B");
        if(native_a && native_b) break;
        pause_poll();
    }
    if(!native_a || !native_b || native_a==native_b) {fprintf(stderr,"Two independent native windows did not appear\n");return 1;}
    close_native(host,native_a);
    for(int i=0;i<500 && named_window(host,"WayX native A");i++) pause_poll();
    if(named_window(host,"WayX native A") || !named_window(host,"WayX native B")) {
        fprintf(stderr,"Closing A affected another native window\n");return 1;
    }
    XCloseDisplay(remote);XCloseDisplay(host);
    puts("Two independent rootless native windows and isolated close behavior verified.");
    return 0;
}
