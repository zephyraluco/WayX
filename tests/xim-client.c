/* A real libX11 client: XOpenIM/XCreateIC/XFilterEvent/Xutf8LookupString. */
#include <X11/Xlib.h>
#include <unistd.h>
#include <X11/Xutil.h>
#include <locale.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static int draws, starts, dones, carets;
static void start(XIM im, XPointer data, XPointer call) {
    (void)im; (void)data; (void)call; starts++;
}
static void done(XIM im, XPointer data, XPointer call) {
    (void)im; (void)data; (void)call; dones++;
}
static void caret(XIM im, XPointer data, XPointer call) {
    (void)im; (void)data;
    XIMPreeditCaretCallbackStruct *p = (void *)call;
    if (p->position == 1) carets++;
}
static void draw(XIM im, XPointer data, XPointer call) {
    (void)im; (void)data;
    XIMPreeditDrawCallbackStruct *p = (void *)call;
    if (p->text && p->text->length) draws++;
}
int main(void) {
    setbuf(stdout, NULL);
    if (!setlocale(LC_ALL, "") || !XSetLocaleModifiers("@im=wayx")) return 2;
    Display *d = XOpenDisplay(NULL);
    if (!d) return 3;
    XIM im = XOpenIM(d, NULL, NULL, NULL);
    if (!im) { fputs("XOpenIM failed\n", stderr); return 4; }
    Window root = DefaultRootWindow(d), w[2];
    XIC ic[2];
    XIMCallback cb_start = {NULL, start}, cb_done = {NULL, done}, cb_draw = {NULL, draw}, cb_caret = {NULL, caret};
    char **missing; int count; char *def;
    XFontSet fonts = XCreateFontSet(d, "fixed", &missing, &count, &def);
    if (missing) XFreeStringList(missing);
    if (!fonts) { fputs("XCreateFontSet failed\n", stderr); return 10; }
    XPoint spot = {25, 40};
    XVaNestedList attrs = XVaCreateNestedList(0, XNFontSet, fonts, XNSpotLocation, &spot,
        XNPreeditStartCallback, &cb_start, XNPreeditDoneCallback, &cb_done,
        XNPreeditDrawCallback, &cb_draw, XNPreeditCaretCallback, &cb_caret, NULL);
    for (int i=0; i<2; i++) {
        w[i] = XCreateSimpleWindow(d, root, 10+i*200, 10, 180, 100, 0, 0, 0);
        XSelectInput(d, w[i], KeyPressMask | StructureNotifyMask);
        XMapWindow(d, w[i]);
        ic[i] = XCreateIC(im, XNInputStyle, (i ? (getenv("WAYX_XIM_POSITION") ? XIMPreeditPosition : XIMPreeditNothing) : XIMPreeditCallbacks) | XIMStatusNothing,
            XNClientWindow, w[i], XNFocusWindow, w[i], XNPreeditAttributes, attrs, NULL);
        if (!ic[i]) { fprintf(stderr, "XCreateIC failed: index=%d position=%d\n", i, getenv("WAYX_XIM_POSITION") != NULL); return 5; }
    }
    XFree(attrs);
    XSetInputFocus(d, w[0], RevertToParent, CurrentTime);
    XSetICFocus(ic[0]); XFlush(d);
    printf("READY %lu %lu\n", w[0], w[1]);
    if (getenv("WAYX_XIM_ABORT")) { sleep(1); _Exit(0); }
    int active = 0;
    for (;;) {
        XEvent event;
        XNextEvent(d, &event);
        if (XFilterEvent(&event, None) || event.type != KeyPress) continue;
        char buf[16384]; KeySym sym; Status status;
        int len = Xutf8LookupString(ic[active], &event.xkey, buf, sizeof(buf)-1, &sym, &status);
        if (len <= 0 || status == XBufferOverflow) return 6;
        buf[len] = 0;
        const char *expected = active ? "第二窗口 한국어 日本語" : "中文输入测试：你好世界。日本語の入力。한국어 입력. This commit is longer than 32 bytes. 😀";
        if (strcmp(buf, expected)) { fprintf(stderr, "Unexpected text: %s\n", buf); return 7; }
        if (!active) {
            if (!draws || !starts || !dones || !carets) return 8;
            char *reset = Xutf8ResetIC(ic[0]);
            if (reset) XFree(reset);
            XUnsetICFocus(ic[0]);
            XPoint next_spot = {50, 60};
            XVaNestedList update = XVaCreateNestedList(0, XNSpotLocation, &next_spot, NULL);
            if (XSetICValues(ic[1], XNPreeditAttributes, update, NULL)) return 9;
            XFree(update);
            XSetInputFocus(d, w[1], RevertToParent, CurrentTime);
            XSetICFocus(ic[1]); XFlush(d);
            active = 1; puts("FOCUS");
        } else {
            XUnsetICFocus(ic[1]); XDestroyIC(ic[0]); XDestroyIC(ic[1]);
            XCloseIM(im); XFreeFontSet(d, fonts); XDestroyWindow(d, w[0]); XDestroyWindow(d, w[1]);
            XCloseDisplay(d); puts("DONE"); return 0;
        }
    }
}
