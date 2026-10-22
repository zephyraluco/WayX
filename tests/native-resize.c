/* SPDX-License-Identifier: MIT
 * Resize the real host window and observe both X root geometry and GPU pixels. */
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
static void pause_poll(void) {struct timespec delay={0,10000000};nanosleep(&delay,NULL);}
static Window desktop(Display *d) {
    Window root,parent,*children=NULL,found=0;unsigned count=0;
    if (!XQueryTree(d,DefaultRootWindow(d),&root,&parent,&children,&count)) return 0;
    for (unsigned i=0;i<count;i++) {
        char *name=NULL;
        if (XFetchName(d,children[i],&name) && name) {
            if (!strcmp(name,"WayX — XLibre desktop")) found=children[i];
            XFree(name);
        }
    }
    if (children) XFree(children);
    return found;
}
int main(int argc,char **argv) {
    if (argc!=3) return 2;
    Display *remote=NULL;
    for (int i=0;i<500 && !remote;i++) {remote=XOpenDisplay(argv[1]);if (!remote) pause_poll();}
    Display *host=XOpenDisplay(argv[2]);
    if (!remote || !host) return 3;
    Window native=0;
    for (int i=0;i<500 && !native;i++) {native=desktop(host);if (!native) pause_poll();}
    if (!native) return 4;
    XResizeWindow(host,native,640,480);XFlush(host);
    Window root=DefaultRootWindow(remote),unused;
    int x,y;unsigned width=0,height=0,border,depth;
    for (int i=0;i<500;i++) {
        XGetGeometry(remote,root,&unused,&x,&y,&width,&height,&border,&depth);
        if (width==640 && height==480) break;
        pause_poll();
    }
    if (width!=640 || height!=480) {fputs("Host resize did not change X root\n",stderr);return 5;}
    Window child=XCreateSimpleWindow(remote,root,600,440,20,20,0,0,0xff0000);
    XMapWindow(remote,child);XSync(remote,False);
    int red=0;
    for (int i=0;i<500;i++) {
        XImage *image=XGetImage(host,native,610,450,1,1,AllPlanes,ZPixmap);
        if (image) {red=(XGetPixel(image,0,0)&0xffffff)==0xff0000;XDestroyImage(image);}
        if (red) break;
        pause_poll();
    }
    XDestroyWindow(remote,child);XCloseDisplay(remote);XCloseDisplay(host);
    if (!red) {fputs("Resized framebuffer was not presented by wgpu\n",stderr);return 6;}
    puts("Native desktop resize changed X root and presented new framebuffer pixels.");
    return 0;
}
