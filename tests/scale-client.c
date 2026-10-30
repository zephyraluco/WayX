/* Verify real scaled native dimensions/pixels/input and bidirectional X11 resize. */
#define _POSIX_C_SOURCE 200809L
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
extern Bool XTestFakeButtonEvent(Display *,unsigned int,Bool,unsigned long);
static void pause_poll(void) {struct timespec delay={0,10000000};nanosleep(&delay,NULL);}
static Window named(Display *d,const char *title) {
    Window root,parent,*children=NULL,found=0;unsigned count=0;
    if(XQueryTree(d,DefaultRootWindow(d),&root,&parent,&children,&count)) {
        for(unsigned i=0;i<count;i++) {char *name=NULL;
            if(XFetchName(d,children[i],&name) && name) {if(!strcmp(name,title))found=children[i];XFree(name);}}
        if(children)XFree(children);
    }
    return found;
}
static int dimensions(Display *d,Window window,unsigned width,unsigned height) {
    for(int i=0;i<500;i++) {XWindowAttributes a;
        if(XGetWindowAttributes(d,window,&a) && (unsigned)a.width==width && (unsigned)a.height==height)return 1;
        pause_poll();}
    XWindowAttributes a;XGetWindowAttributes(d,window,&a);
    fprintf(stderr,"Size mismatch: got %dx%d expected %ux%u\n",a.width,a.height,width,height);return 0;
}
int main(int argc,char **argv) {
    if(argc!=5)return 2;
    double scale=strtod(argv[3],NULL);int rootless=atoi(argv[4]);
    Display *remote=NULL;
    for(int i=0;i<500 && !remote;i++){remote=XOpenDisplay(argv[1]);if(!remote)pause_poll();}
    Display *host=XOpenDisplay(argv[2]);if(!remote || !host)return 3;
    Window root=DefaultRootWindow(remote),child=XCreateSimpleWindow(remote,root,20,20,160,120,0,0,0);
    XStoreName(remote,child,"WayX scale validation");
    XSelectInput(remote,child,StructureNotifyMask|PointerMotionMask|ButtonPressMask);
    XSizeHints hints={0};hints.flags=PMinSize|PMaxSize|PResizeInc;
    hints.min_width=100;hints.min_height=80;hints.max_width=400;hints.max_height=300;
    hints.width_inc=5;hints.height_inc=5;XSetWMNormalHints(remote,child,&hints);
    XMapWindow(remote,child);XEvent event;
    do{XNextEvent(remote,&event);}while(event.type!=MapNotify);
    GC gc=XCreateGC(remote,child,0,NULL);XSetForeground(remote,gc,0xff0000);XFillRectangle(remote,child,gc,0,0,160,120);
    XSetForeground(remote,gc,0x0000ff);XFillRectangle(remote,child,gc,10,10,20,20);XSync(remote,False);
    Window native=0;
    for(int i=0;i<500 && !native;i++){native=named(host,rootless?"WayX scale validation":"WayX — XLibre desktop");if(!native)pause_poll();}
    if(!native)return 4;
    if(!dimensions(host,native,(unsigned)lround((rootless?160:320)*scale),(unsigned)lround((rootless?120:240)*scale)))return 5;
    if(!dimensions(remote,child,160,120))return 6;
    int offset=rootless?0:20,painted=0;
    for(int i=0;i<500;i++) {
        XImage *image=XGetImage(host,native,0,0,(unsigned)lround((rootless?160:320)*scale),(unsigned)lround((rootless?120:240)*scale),AllPlanes,ZPixmap);
        if(image){unsigned long red=XGetPixel(image,(int)((offset+60)*scale),(int)((offset+40)*scale))&0xffffff;
            unsigned long blue=XGetPixel(image,(int)((offset+15)*scale),(int)((offset+15)*scale))&0xffffff;
            XDestroyImage(image);painted=red==0xff0000 && blue==0x0000ff;}
        if(painted)break;
        pause_poll();
    }
    if(!painted){fputs("Scaled native pixels missing\n",stderr);return 7;}
    XWarpPointer(host,None,native,0,0,0,0,(int)lround((offset+60)*scale),(int)lround((offset+40)*scale));XFlush(host);
    int input_ok=0;
    for(int i=0;i<500;i++){Window unused1,unused2;int rx,ry,x,y;unsigned mask;
        XQueryPointer(remote,child,&unused1,&unused2,&rx,&ry,&x,&y,&mask);
        if(abs(x-60)<=1 && abs(y-40)<=1){input_ok=1;break;}pause_poll();}
    if(!input_ok){fputs("Scaled pointer coordinates did not match X11 pixels\n",stderr);return 8;}
    XTestFakeButtonEvent(host,1,True,CurrentTime);
    XTestFakeButtonEvent(host,1,False,CurrentTime);XSync(host,False);
    int clicked=0;
    for(int i=0;i<500 && !clicked;i++) {
        while(XPending(remote)) {XNextEvent(remote,&event);
            if(event.type==ButtonPress && event.xbutton.button==1 && abs(event.xbutton.x-60)<=1 && abs(event.xbutton.y-40)<=1)clicked=1;}
        if(!clicked)pause_poll();
    }
    if(!clicked){fputs("Scaled button coordinates did not match X11 pixels\n",stderr);return 11;}
    XResizeWindow(host,native,(unsigned)lround((rootless?200:400)*scale),(unsigned)lround((rootless?150:300)*scale));XFlush(host);
    if(!dimensions(remote,rootless?child:root,rootless?200:400,rootless?150:300))return 9;
    if(rootless){XResizeWindow(remote,child,180,130);XFlush(remote);
        if(!dimensions(host,native,(unsigned)lround(180*scale),(unsigned)lround(130*scale)))return 10;}
    XFreeGC(remote,gc);XDestroyWindow(remote,child);XCloseDisplay(remote);XCloseDisplay(host);
    printf("scale=%g rootless=%d: native pixels, pointer, click and bidirectional geometry passed\n",scale,rootless);return 0;
}
