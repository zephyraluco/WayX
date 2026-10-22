/* SPDX-License-Identifier: MIT
 * Reuse XLibre's FB input device initialization; retain upstream source/license. */
#define InitInput WayxVfbInitInput
#include "../vfb/InitInput.c"
#undef InitInput

/* XI2 scroll axes retain fractional deltas. XLibre also accumulates and
 * emulates buttons 4-7 for clients which only select core pointer events. */
static int wayxMouseProc(DeviceIntPtr device, int what) {
    if (what != DEVICE_INIT) return vfbMouseProc(device, what);
    BYTE map[14];
    Atom buttons[13] = {0};
    Atom axes[4] = {
        XIGetKnownProperty(AXIS_LABEL_PROP_REL_X),
        XIGetKnownProperty(AXIS_LABEL_PROP_REL_Y),
        XIGetKnownProperty(AXIS_LABEL_PROP_REL_HSCROLL),
        XIGetKnownProperty(AXIS_LABEL_PROP_REL_VSCROLL)
    };
    for (int i=0;i<14;i++) map[i]=i;
    if (!InitPointerDeviceStruct((DevicePtr)device, map, 13, buttons,
        (PtrCtrlProcPtr)NoopDDA, GetMotionHistorySize(), 4, axes)) return BadAlloc;
    for (int i=0;i<4;i++)
        InitValuatorAxisStruct(device, i, axes[i], -1, -1, 0, 0, 0, Relative);
    SetScrollValuator(device, 2, SCROLL_TYPE_HORIZONTAL, 1.0, SCROLL_FLAG_NONE);
    SetScrollValuator(device, 3, SCROLL_TYPE_VERTICAL, -1.0, SCROLL_FLAG_PREFERRED);
    return Success;
}
void InitInput(int argc,char **argv);
DeviceIntPtr wayx_pointer, wayx_keyboard;
void InitInput(int argc, char **argv) {
    WayxVfbInitInput(argc, argv);
    DeviceIntPtr lists[]={inputInfo.devices,inputInfo.off_devices};
    for (unsigned int i=0;i<2;i++) for (DeviceIntPtr d=lists[i]; d; d=d->next) {
        if (d->deviceProc == vfbMouseProc) { wayx_pointer=d; d->deviceProc=wayxMouseProc; }
        if (d->deviceProc == vfbKeybdProc) wayx_keyboard=d;
    }
    if (!wayx_pointer || !wayx_keyboard) FatalError("WayX input devices missing\n");
}
