/* SPDX-License-Identifier: MIT
 * Dynamic software screen storage. All work runs on the XLibre thread. */
static Bool wayx_screen_size(ScreenPtr s, CARD16 width, CARD16 height,
                             CARD32 mm_width, CARD32 mm_height) {
    if ((rootless && !layout_applying) || !width || !height || width>16384 || height>16384 ||
        (uint64_t)width*height*4>256u*1024u*1024u ||
        !mm_width || !mm_height || mm_width>0x7fffffff || mm_height>0x7fffffff ||
        fbmemtype!=NORMAL_MEMORY_FB)
        return FALSE;
    vfbScreenInfoPtr fb=vfbGetScreenPriv(s);
    if (width==s->width && height==s->height) {
        s->mmWidth=mm_width;s->mmHeight=mm_height;
        RRScreenSizeNotify(s);
        return TRUE;
    }
    vfbScreenInfo replacement=*fb;
    replacement.width=width;replacement.height=height;
    replacement.paddedWidth=width;replacement.paddedBytesWidth=width*4;
    replacement.pfbMemory=NULL;replacement.pXWDHeader=NULL;
    if (!vfbAllocateFramebufferMemory(&replacement)) return FALSE;
    unsigned rows=height<fb->height?height:fb->height;
    size_t bytes=(width<fb->width?width:fb->width)*4;
    for (unsigned y=0;y<rows;y++)
        memcpy(replacement.pfbMemory+y*replacement.paddedBytesWidth,
               fb->pfbMemory+y*fb->paddedBytesWidth,bytes);
    PixmapPtr pixmap=s->GetScreenPixmap(s);
    SetRootClip(s,ROOT_CLIP_NONE);
    if (!s->ModifyPixmapHeader(pixmap,width,height,-1,-1,
                              replacement.paddedBytesWidth,replacement.pfbMemory)) {
        SetRootClip(s,ROOT_CLIP_FULL);
        free(replacement.pXWDHeader);return FALSE;
    }
    if (!rootless && api->window_resize(backend,1,width,height)<0) {
        s->ModifyPixmapHeader(pixmap,fb->width,fb->height,-1,-1,
                              fb->paddedBytesWidth,fb->pfbMemory);
        SetRootClip(s,ROOT_CLIP_FULL);
        free(replacement.pXWDHeader);return FALSE;
    }
    XWDFileHeader *old=fb->pXWDHeader;
    *fb=replacement;
    s->width=width;s->height=height;s->mmWidth=mm_width;s->mmHeight=mm_height;
    vfbWriteXWDFileHeader(s);
    SetRootClip(s,ROOT_CLIP_FULL);
    RRScreenSizeNotify(s);
    if (!rootless && api->frame_damage(backend,1,0,0,width,height,
            fb->paddedBytesWidth,(const uint8_t*)fb->pfbMemory,
            (size_t)fb->paddedBytesWidth*height)<0)
        FatalError("WayX resized framebuffer upload rejected\n");
    free(old);
    return TRUE;
}

static void resize_desktop(unsigned width,unsigned height) {
    rrScrPrivPtr rr=rrGetScrPriv(screen);
    /* Native resize controls a single desktop output. Multi-output layouts
     * remain under RandR client control instead of silently moving monitors. */
    if (!width || !height || width>16384 || height>16384 || rr->numCrtcs!=1 ||
        (width==screen->width && height==screen->height)) return;
    xRRModeInfo info={0};char name[64];
    snprintf(name,sizeof(name),"%ux%u",width,height);
    info.width=width;info.height=height;info.nameLength=strlen(name);
    RRModePtr mode=RRModeGet(&info,name);
    if (!mode) return;
    CARD32 mm_width=(uint64_t)width*254/960,mm_height=(uint64_t)height*254/960;
    if (!wayx_screen_size(screen,width,height,mm_width?mm_width:1,mm_height?mm_height:1)) {
        RRModeDestroy(mode);return;
    }
    RRCrtcPtr crtc=rr->crtcs[0];
    if (rr->numOutputs) {
        RROutputPtr output=rr->outputs[0];
        /* RROutputSetModes takes ownership of RRModeGet's reference. */
        if (!RROutputSetModes(output,&mode,1,1)) {RRModeDestroy(mode);return;}
        RROutputSetPhysicalSize(output,screen->mmWidth,screen->mmHeight);
        vfbRRCrtcSet(screen,crtc,mode,0,0,RR_Rotate_0,1,&output);
    }
    else RRModeDestroy(mode);
    RRTellChanged(screen);
}

/* Separate from rootful's single-CRTC desktop resize. Only host layout events
 * enter here; moving or resizing a top-level window never changes the screen. */
static vfbCrtcInfo hotplug_crtcs[WAYX_MAX_MONITORS];
static int configure_native_window(WindowPtr,Mask,XID*);
static void apply_host_layout(void) {
    WayxDesktopLayout next={0};
    if (api->desktop_layout(backend,&next)!=1) FatalError("WayX: missing host desktop layout\n");
    if (host_layout_active && !memcmp(&next,&host_layout,sizeof(next))) return;
    rrScrPrivPtr rr=rrGetScrPriv(screen);
    RRModePtr modes[WAYX_MAX_MONITORS]={0};
    for (unsigned i=0;i<next.count;i++) {
        xRRModeInfo info={0};char name[64];WayxMonitorRect m=next.monitors[i];
        snprintf(name,sizeof(name),"%ux%u",m.width,m.height);
        info.width=m.width;info.height=m.height;info.nameLength=strlen(name);
        modes[i]=RRModeGet(&info,name);
        if (!modes[i]) FatalError("WayX: cannot prepare host display mode\n");
    }
    /* Keep removed outputs disconnected so clients retain valid resource IDs. */
    while (rr->numCrtcs<(int)next.count) {
        unsigned i=rr->numCrtcs;char name[64];
        RRCrtcPtr crtc=RRCrtcCreate(screen,&hotplug_crtcs[i]);
        snprintf(name,sizeof(name),"screen_%u",i);
        RROutputPtr output=RROutputCreate(screen,name,strlen(name),NULL);
        if (!crtc || !output || !RRCrtcGammaSetSize(crtc,256) ||
            !RROutputSetClones(output,NULL,0) || !RROutputSetCrtcs(output,&crtc,1))
            FatalError("WayX: cannot create host display output\n");
    }
    for (int i=0;i<rr->numCrtcs;i++)
        if (!vfbRRCrtcSet(screen,rr->crtcs[i],NULL,0,0,RR_Rotate_0,0,NULL))
            FatalError("WayX: cannot disable previous desktop layout\n");
    layout_applying=TRUE;
    CARD32 mmw=(uint64_t)next.width*254/960,mmh=(uint64_t)next.height*254/960;
    if (!wayx_screen_size(screen,next.width,next.height,mmw?mmw:1,mmh?mmh:1))
        FatalError("WayX: cannot allocate host desktop framebuffer\n");
    layout_applying=FALSE;
    int64_t dx=(int64_t)desktop_origin_x-next.origin_x,dy=(int64_t)desktop_origin_y-next.origin_y;
    desktop_origin_x=next.origin_x;desktop_origin_y=next.origin_y;
    for (unsigned i=0;i<(unsigned)rr->numOutputs;i++) {
        RROutputPtr output=rr->outputs[i];
        if (i<next.count) {
            WayxMonitorRect m=next.monitors[i];
            if (!RROutputSetModes(output,&modes[i],1,1) || !RROutputSetConnection(output,RR_Connected) ||
                !RROutputSetPhysicalSize(output,(uint64_t)m.width*254/960,(uint64_t)m.height*254/960) ||
                !vfbRRCrtcSet(screen,rr->crtcs[i],modes[i],m.x-next.origin_x,m.y-next.origin_y,RR_Rotate_0,1,&output))
                FatalError("WayX: cannot apply host display output\n");
        } else if (!RROutputSetConnection(output,RR_Disconnected)) FatalError("WayX: cannot disconnect display\n");
    }
    /* Preserve host positions, including off-screen windows; only protocol
     * representability limits apply, not whole-window containment. */
    for (WindowPtr window=screen->root->firstChild;window;) {
        WindowPtr sibling=window->nextSib;
        int64_t x=(int64_t)window->drawable.x-window->borderWidth+dx;
        int64_t y=(int64_t)window->drawable.y-window->borderWidth+dy;
        XID values[]={x<-32768?-32768:x>32767?32767:x,y<-32768?-32768:y>32767?32767:y};
        configure_native_window(window,CWX|CWY,values);
        window=sibling;
    }
    host_layout=next;host_layout_active=TRUE;root_geometry_pending=TRUE;metadata_dirty=TRUE;
    RRScreenSizeNotify(screen);RRTellChanged(screen);
    ErrorF("WayX host desktop layout applied: %ux%u origin %d,%d\n",next.width,next.height,next.origin_x,next.origin_y);
}

static Bool reject_rootless_crtc(ScreenPtr s,RRCrtcPtr crtc,RRModePtr mode,int x,int y,
                                 Rotation rotation,int numOutputs,RROutputPtr *outputs) {
    return FALSE;
}
