#!/usr/bin/env python3
"""Create a new local Xauthority without printing its cookie or overwriting files."""
import argparse
import os
import secrets
import socket
import struct
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--display", type=int, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
if not 0 <= args.display <= 59535:
    parser.error("display must be between 0 and 59535")
# FamilyLocal, host, display, MIT authentication name and cryptographic cookie.
data = struct.pack(">H", 256)
for field in (socket.gethostname().encode(), str(args.display).encode(),
              b"MIT-MAGIC-COOKIE-1", secrets.token_bytes(16)):
    data += struct.pack(">H", len(field)) + field
fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "wb") as output:
    output.write(data)
print(f"Created {args.output}; configure the X server with -auth and keep this file private.")
