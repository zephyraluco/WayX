/* Exercise redraw after an idle interval without a continuous animation. */
#include <X11/Xlib.h>
#include <stdio.h>
#include <stdlib.h>
int main(int argc,char **argv) {
    Display *display=XOpenDisplay(argc>1?argv[1]:NULL);
    if(!display) return 1;
    Window window=XCreateSimpleWindow(display,DefaultRootWindow(display),20,20,200,160,0,0,0);
    GC gc=XCreateGC(display,window,0,NULL);
    XStoreName(display,window,"WayX idle regression");
    XSelectInput(display,window,StructureNotifyMask);
    XMapWindow(display,window);
    /* An external WM may defer MapWindow; drawing before MapNotify can be lost. */
    XEvent event;
    do { XNextEvent(display,&event); } while(event.type!=MapNotify);
    for(unsigned long color=0xff0000; ;color=0x00ff00) {
        XSetForeground(display,gc,color);
        XFillRectangle(display,window,gc,0,0,200,160);
        XSync(display,False);
        puts("DRAWN");fflush(stdout);
        if(getchar()!='d') break;
    }
    XFreeGC(display,gc);XDestroyWindow(display,window);XCloseDisplay(display);
    return 0;
}
