#!/usr/bin/env python3
"""Prepare local precompiled fixtures for bundled-runtime integration tests."""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
from package_support import stamp, archive, digest, elf_dependencies

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--runtime", type=Path, required=True)
parser.add_argument("--archive-dir", type=Path)
parser.add_argument("--integration-only", action="store_true")
parser.add_argument("--profile", choices=("release", "debug"), default="release")
args = parser.parse_args()
out = args.output.resolve()
out.mkdir(parents=True, exist_ok=False)
(out/"bin").mkdir()
executables, units = {}, []
profile = ["--release"] if args.profile == "release" else []
suffix = ".exe" if sys.platform == "win32" else ""


def stage(name, source):
    destination = out/"bin"/(name+suffix)
    shutil.copy2(source, destination)
    executables[name] = destination.relative_to(out).as_posix()


def cargo_tests(extra, core_only=False):
    command = ["cargo", "test", "--locked", *profile, *extra, "--no-run", "--message-format=json"]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
    for line in process.stdout:
        message = json.loads(line)
        if message.get("reason") == "compiler-message":
            print(message["message"].get("rendered", ""), end="", flush=True)
        if message.get("reason") != "compiler-artifact" or not message.get("executable") or not message["profile"]["test"]:
            continue
        target = message["target"]
        name = "unit-core" if core_only else (target["name"] if "test" in target["kind"] else "unit-"+target["kind"][0]+"-"+target["name"])
        stage(name, message["executable"])
        if core_only or "test" not in target["kind"]: units.append(name)
    if process.wait() != 0: raise RuntimeError("Test compilation failed")


if args.integration_only:
    targets = ["xlibre", "rootless", "window_manager", "desktop_layout", "ime", "glx", "native_geometry", "native_gpu", "local_display"]
    cargo_tests([item for target in targets for item in ("--test", target)])
else:
    cargo_tests(["--all-targets"])
    cargo_tests(["--no-default-features", "--lib"], core_only=True)
# Existing C ABI coverage is Linux-specific; no test is run during packaging.
if sys.platform == "linux":
    subprocess.run(["cargo", "build", "--locked", *profile, "--no-default-features", "--lib"], check=True)
    library = Path(os.environ.get("CARGO_TARGET_DIR", "target"))/args.profile/"libwayx.a"
    destination = out/"bin"/"c-bridge"
    subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-Iinclude", "tests/bridge.c", str(library),
                    "-ldl", "-lpthread", "-lm", "-lrt", "-lutil", "-o", str(destination)], check=True)
    executables["c-bridge"] = "bin/c-bridge"


def compile_c(name, source, flags=(), cxx=False):
    destination = out/"bin"/(name+suffix)
    subprocess.run(["c++" if cxx else "cc", "-std=c++17" if cxx else "-std=c11", "-Wall", "-Wextra", "-Werror",
                    *([] if cxx else ["-D_POSIX_C_SOURCE=200809L"]), source, *flags, "-o", str(destination)], check=True)
    executables[name] = destination.relative_to(out).as_posix()


def pkg(*packages, option="--cflags", libs=True):
    return shlex.split(subprocess.check_output(["pkg-config", option, *(["--libs"] if libs else []), *packages], text=True))


if sys.platform in ("linux", "darwin"):
    compile_c("xim-client", "tests/xim-client.c", pkg("x11"))
    compile_c("idle-client", "tests/idle-client.c", pkg("x11"))
if sys.platform == "linux":
    for name in ("native-resize", "native-rootless", "native-glx"):
        compile_c(name, "tests/"+name+".c", pkg("x11"))
    compile_c("glx-client", "tests/glx-client.c", pkg("gl", "x11"))
    compile_c("scale-client", "tests/scale-client.c", [*pkg("x11"), "-lm", "-l:libXtst.so.6"])
    includedir = subprocess.check_output(["pkg-config", "--variable=includedir", "OGRE"], text=True).strip()
    compile_c("ogre-client", "tests/ogre-client.cpp", ["-isystem", includedir+"/OGRE", *pkg("OGRE", option="--libs", libs=False)], cxx=True)

if sys.platform == "linux":
    # External C clients carry their own non-system libraries, isolated by RUNPATH.
    # Rust protocol harnesses and the product never search this sidecar directory.
    libdir = out/"lib"; libdir.mkdir()
    plugin_dir = Path(subprocess.check_output(["pkg-config", "--variable=plugindir", "OGRE"], text=True).strip())
    plugin = plugin_dir/"RenderSystem_GL.so"
    shutil.copy2(plugin, libdir/plugin.name)
    executables["ogre-plugin"] = "lib/"+plugin.name
    queue = [out/value for name,value in executables.items() if name in {"c-bridge", "xim-client", "glx-client", "ogre-client", "ogre-plugin", "native-resize", "native-rootless", "native-glx", "idle-client", "scale-client"}]
    staged = {}
    while queue:
        binary = queue.pop()
        for dependency, system in elf_dependencies(binary):
            if system: continue
            source = dependency.resolve()
            if dependency.name in staged:
                if staged[dependency.name] != source: raise RuntimeError("Conflicting test-client dependency: "+dependency.name)
                continue
            staged[dependency.name] = source
            target = libdir/dependency.name
            shutil.copy2(source, target); queue.append(target)
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN" if binary.parent==libdir else "$ORIGIN/../lib", str(binary)], check=True)
shutil.copytree(args.runtime/"licenses", out/"licenses")
shutil.copytree("scripts", out/"scripts", ignore=shutil.ignore_patterns("__pycache__"))
# Test jobs need no checkout or build source, and cannot fall back to compilation.
# Only test drivers, the fixture resolver and manifest verifier are shipped.
keep = {"test-package.py", "test-package-support.py", "test-native.py", "test-idle-cpu.py", "test-macos-lifecycle.py", "test-scale.py", "test_fixtures.py", "create-xauthority.py", "package_support.py", "relocate-runtime.py", "prepare-release.py"}
for path in (out/"scripts").iterdir():
    if path.name not in keep:
        if path.is_dir(): shutil.rmtree(path)
        else: path.unlink()
stamp(out, "test-kit", profile=args.profile, executables=executables, units=units, runtime_manifest_sha256=digest(args.runtime/"manifest.json"))
if args.archive_dir: archive(out, args.archive_dir, "core-test-kit")
