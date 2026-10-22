/* SPDX-License-Identifier: MIT
 * Dynamic software screen storage. All work runs on the XLibre thread. */
static Bool wayx_screen_size(ScreenPtr s, CARD16 width, CARD16 height,
                             CARD32 mm_width, CARD32 mm_height) {
    if (!width || !height || width>16384 || height>16384 ||
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
    if (api->window_resize(backend,1,width,height)<0) {
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
