#!/usr/bin/env python3
"""Prepare pinned XLibre GLX for client-side rendering, without a host GL library.

All server-side indirect contexts are rejected by wayx-glx.c. Keep the upstream
wire decoder and route its GL calls through the existing procedure dispatcher;
this avoids linking unused OpenGL implementations into the portable server.
"""
from pathlib import Path
import re
import shutil
import sys

source, headers = map(lambda value: Path(value).resolve(), sys.argv[1:])
glx = source / "Xext/glx"
functions = sorted({name for path in glx.glob("*.c")
                    for name in re.findall(r"\b(gl[A-Z]\w*)\s*\(", path.read_text())
                    if not name.startswith("glX")})
if not functions or not (headers / "GL/gl.h").is_file():
    raise SystemExit("Missing GLX sources or OpenGL headers")
header = source / "hw/wayx/wayx-gl-dispatch.h"
text = """/* Generated for the client-only WayX GLX provider. */
#include <dix-config.h>
#define GL_GLEXT_PROTOTYPES 1
#include <GL/gl.h>
#include <GL/glext.h>
extern void *__glGetProcAddress(const char *);
"""
for name in functions:
    text += f'#define {name}(...) ((__typeof__(&{name}))__glGetProcAddress("{name}"))(__VA_ARGS__)\n'
header.write_text(text)
path = glx / "meson.build"
text = path.read_text()
old = "dependency('gl', version: '>= 1.2')"
new = "declare_dependency(include_directories: include_directories('../../hw/wayx/gl-headers'))"
if old in text:
    if text.count(old) != 2:
        raise SystemExit("Unexpected pinned XLibre GL dependencies")
    text = text.replace(old, new)
elif new not in text:
    raise SystemExit("Missing pinned XLibre GL dependencies")
old = "        c_args: [\n            glx_align64,"
new = "        c_args: [\n            '-include', meson.current_source_dir() / '../../hw/wayx/wayx-gl-dispatch.h',\n            glx_align64,"
if old in text:
    text = text.replace(old, new, 1)
elif new not in text:
    raise SystemExit("Unexpected pinned XLibre GL dispatch arguments")
path.write_text(text)
# Reject attempts to send server GL commands using a client-rendered context.
# The stock provider has a host GL dispatch table; WayX deliberately has none.
path = glx / "glxext.c"
text = path.read_text()
old = """    /* If we're expecting a glXRenderLarge request, this better be one. */"""
new = """    if (cx->isDirect) {
        *error = __glXError(GLXBadContextState);
        return NULL;
    }

""" + old
if new not in text:
    if text.count(old) != 1:
        raise SystemExit("Unexpected pinned XLibre context validation")
    path.write_text(text.replace(old, new, 1))
# Copy only Khronos/libglvnd headers; retain their bundled copyright notices.
shutil.copytree(headers, source / "hw/wayx/gl-headers", dirs_exist_ok=True)

# Only ABI declarations are needed. Prevent GL headers from pulling Win32 UI
# types (BOOL/ATOM/INT32/FreeResource) into the X server namespace.
private_gl = source / "hw/wayx/gl-headers/GL/gl.h"
private_gl.write_text("""/* WayX: declare APIENTRY without importing windows.h into XLibre. */
#if defined(_WIN32) && !defined(APIENTRY)
#define APIENTRY __stdcall
#endif
""" + private_gl.read_text())
