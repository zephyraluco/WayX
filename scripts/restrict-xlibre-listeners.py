#!/usr/bin/env python3
"""Bind the dedicated WayX core's IPv4/IPv6 X11 listeners to loopback only."""
from pathlib import Path
import sys

path = Path(sys.argv[1])/"os"/"Xtranssock.c"
text = path.read_text()
for original, replacement in (
    ("sin_addr.s_addr = htonl(INADDR_ANY);", "sin_addr.s_addr = htonl(INADDR_LOOPBACK);"),
    ("sin6_addr = in6addr_any;", "sin6_addr = in6addr_loopback;"),
):
    if text.count(original) == 1 and replacement not in text:
        text = text.replace(original, replacement)
    elif original in text or text.count(replacement) != 1:
        raise RuntimeError("Unexpected pinned XLibre listener code: " + original)
path.write_text(text)
