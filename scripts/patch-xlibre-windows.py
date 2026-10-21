#!/usr/bin/env python3
"""Apply checked LLP64 pointer conversions to the pinned XLibre source."""
from pathlib import Path
import sys

root = Path(sys.argv[1])
patches = {
    "dix/events.c": [("(WindowPtr) (long) focusID", "(WindowPtr) (intptr_t) focusID")],
    "dix/pixmap.c": [("(void *)(long)ihandle", "(void *)(intptr_t)ihandle")],
    "Xext/record/set.c": [
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
        if new in text:
            continue
        if text.count(old) != 1:
            raise SystemExit(f"Unexpected pinned XLibre pointer conversion: {filename}")
        text = text.replace(old, new, 1)
    path.write_text(text)
