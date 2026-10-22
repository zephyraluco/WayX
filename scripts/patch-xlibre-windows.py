#!/usr/bin/env python3
"""Apply checked Windows compatibility fixes to the pinned XLibre source."""
from pathlib import Path
import sys

root = Path(sys.argv[1])
patches = {
    "os/Xtranssock.c": [("#include <afunix.h>\n\n#undef EADDRINUSE", "#include <afunix.h>\n#include <process.h>\n\n#undef EADDRINUSE")],
    # The server transport uses no Xlib internals. Importing them after
    # Xwindows.h conflicts with its Status macro and leaks client-side types.
    "os/Xtransutil.c": [("#include <X11/Xlibint.h>\n", "/* Server transport does not require Xlib internals. */\n")],
    "dix/events.c": [("(WindowPtr) (long) focusID", "(WindowPtr) (intptr_t) focusID")],
    "dix/resource.c": [
        ("AddResource(%lx, %x, %lx)", "AddResource(%lx, %x, %p)"),
        ("(unsigned long) id, type, (unsigned long) value, client", "(unsigned long) id, type, value, client"),
    ],
    "mi/miscrinit.c": [("(unsigned long *) (unsigned long) bias", "(void *) (uintptr_t) bias")],
    "include/miline.h": [("((unsigned long) (unsigned long*)", "((uintptr_t)")],
    "fb/fbsetsp.c": [("(long) src", "(uintptr_t) src")],
    "fb/fbgetsp.c": [("(long) pchardstStart", "(uintptr_t) pchardstStart")],
    "miext/shadow/sh3224.c": [("(long)dst", "(uintptr_t)dst")],
    "dix/pixmap.c": [("(void *)(long)ihandle", "(void *)(intptr_t)ihandle")],
    "Xext/record/set.c": [
        ("(long) pMem", "(uintptr_t) pMem"),
        ("(int) (long) pIter", "(int) (intptr_t) pIter"),
        ("(RecordSetIteratePtr) (long) (pInterval->last + 1)",
         "(RecordSetIteratePtr) (intptr_t) (pInterval->last + 1)"),
    ],
    "fb/fboverlay.c": [("(void *) (long) i", "(void *) (intptr_t) i")],
}
for filename, replacements in patches.items():
    path = root / filename
    text = path.read_text()
    for old, new in replacements:
        # Require complete, distinct expressions: substring matches can conceal
        # an unapplied fix or insert the same header again on a second run.
        if old in new or new in old:
            raise SystemExit(f"Ambiguous Windows patch expression: {filename}")
        count = text.count(old)
        if count == 1:
            text = text.replace(old, new, 1)
        elif count != 0 or new not in text:
            raise SystemExit(f"Unexpected pinned XLibre Windows patch: {filename}")
    path.write_text(text)
