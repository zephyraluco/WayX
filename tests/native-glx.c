/* SPDX-License-Identifier: MIT
 * Observe the actual host window after X11 software GL -> Damage -> wgpu. */
#define _POSIX_C_SOURCE 200809L
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
static int ignore_stale(Display *d,XErrorEvent *event) {(void)d;(void)event;return 0;}
int main(void) {
    Display *d=XOpenDisplay(NULL);
    if(!d) return 2;
    XSetErrorHandler(ignore_stale);
    for(int attempt=0;attempt<200;attempt++) {
        Window root,parent,*windows=NULL;unsigned count=0;
        if(XQueryTree(d,DefaultRootWindow(d),&root,&parent,&windows,&count)) {
            for(unsigned i=0;i<count;i++) {
                char *name=NULL;
                if(!XFetchName(d,windows[i],&name) || !name) continue;
                int matches=!strcmp(name,"WayX GLX validation");XFree(name);
                if(!matches) continue;
                XWindowAttributes attrs;
                if(!XGetWindowAttributes(d,windows[i],&attrs) || attrs.map_state!=IsViewable || attrs.width<4 || attrs.height<4) continue;
                XImage *image=XGetImage(d,windows[i],0,0,attrs.width,attrs.height,AllPlanes,ZPixmap);
                if(!image) continue;
                unsigned long center=XGetPixel(image,attrs.width/2,attrs.height/2)&0xffffff;
                unsigned long corner=XGetPixel(image,2,2)&0xffffff;
                XDestroyImage(image);
                if(center==0xff0000 && corner==0x0000ff) {
                    XFree(windows);XCloseDisplay(d);
                    puts("Native GLX red triangle and blue background presented by wgpu");return 0;
                }
            }
            if(windows) XFree(windows);
        }
        struct timespec delay={0,50000000};nanosleep(&delay,NULL);
    }
    fputs("Native GLX pixels did not appear\n",stderr);XCloseDisplay(d);return 3;
}
