/* SPDX-License-Identifier: MIT
 * Reuse XLibre's FB input device initialization; retain upstream source/license. */
#define InitInput WayxVfbInitInput
#include "../vfb/InitInput.c"
#undef InitInput

void InitInput(int argc,char **argv);
DeviceIntPtr wayx_pointer, wayx_keyboard;
void InitInput(int argc, char **argv) {
    WayxVfbInitInput(argc, argv);
    DeviceIntPtr lists[]={inputInfo.devices,inputInfo.off_devices};
    for (unsigned int i=0;i<2;i++) for (DeviceIntPtr d=lists[i]; d; d=d->next) {
        if (d->deviceProc == vfbMouseProc) wayx_pointer=d;
        if (d->deviceProc == vfbKeybdProc) wayx_keyboard=d;
    }
    if (!wayx_pointer || !wayx_keyboard) FatalError("WayX input devices missing\n");
}
