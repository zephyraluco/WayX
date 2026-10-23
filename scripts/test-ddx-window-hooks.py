#!/usr/bin/env python3
"""Compile the real DDX with Windows' renamed Screen.CreateWindow member on Linux."""
import argparse
import json
from pathlib import Path
import shlex
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("--build", type=Path, default=Path("build/xlibre"))
args = parser.parse_args()
commands = json.loads((args.build / "compile_commands.json").read_text())
entry = next(item for item in commands if item["file"].endswith("hw/wayx/wayx-output.c"))
original = entry.get("arguments") or shlex.split(entry["command"])
command = []
# Syntax checking must not replace Ninja's object or dependency files.
index = 0
while index < len(original):
    argument = original[index]
    if argument in ("-o", "-MF", "-MQ", "-MT"):
        index += 2
    elif argument in ("-c", "-MD", "-MMD", "-MP"):
        index += 1
    else:
        command.append(argument)
        index += 1
with tempfile.TemporaryDirectory(prefix="wayx-screen-member-") as directory:
    header = Path(directory) / "windows-screen-member.h"
    header.write_text("""#include <dix-config.h>
#define CreateWindow CreateWindowA
#include "scrnintstr.h"
#undef CreateWindow
#define WAYX_CREATE_WINDOW_MEMBER CreateWindowA
""")
    subprocess.run(command + ["-fsyntax-only", "-include", str(header)],
                   cwd=entry["directory"], check=True)
print("DDX compiles with Windows' CreateWindowA field and no CreateWindow macro.")
