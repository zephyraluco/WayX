#!/usr/bin/env python3
"""Stage a native core runtime with resolved C dependencies and relocatable XKB data."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--core", type=Path, required=True)
parser.add_argument("--client", type=Path, required=True)
parser.add_argument("--compiler", type=Path, required=True)
parser.add_argument("--xkb-dir", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--search-dir", type=Path, action="append", default=[])
parser.add_argument("--license-dir", type=Path, action="append", default=[])
args = parser.parse_args()
if sys.platform not in ("darwin", "win32"):
    parser.error("native macOS or Windows is required")
for directory in args.license_dir:
    if not directory.is_dir():
        parser.error(f"missing license source: {directory}")
out = args.output.resolve()
if out.exists():
    parser.error("output already exists; choose a new staging directory")
for file in (args.core, args.client, args.compiler):
    if not file.is_file():
        parser.error(f"missing binary: {file}")
if not (args.xkb_dir / "rules" / "evdev").is_file():
    parser.error("XKB directory must contain generated rules/evdev")
bindir = out / "bin"
libdir = bindir if sys.platform == "win32" else out / "lib"
bindir.mkdir(parents=True)
libdir.mkdir(exist_ok=True)
search = [path.resolve() for path in args.search_dir]
if sys.platform == "win32" and os.environ.get("VCToolsRedistDir"):
    search += list((Path(os.environ["VCToolsRedistDir"]) / "x64").glob("Microsoft.VC*.CRT"))
objects = {}
queue = []
system = set()


def register(source, destination):
    """Reject same-name collisions rather than silently packaging the wrong ABI."""
    source = source.resolve()
    key = str(destination).casefold()
    if key in objects:
        if objects[key][0] != source:
            raise RuntimeError(f"conflicting runtime libraries: {source}, {objects[key][0]}")
        return destination
    shutil.copy2(source, destination)
    objects[key] = (source, destination)
    queue.append((source, destination))
    return destination


def output(*command):
    return subprocess.check_output(command, text=True, encoding="utf-8")


def resolve_macho(name, source):
    if name.startswith(("/usr/lib/", "/System/Library/")):
        system.add(name)
        return None
    if name.startswith("@loader_path/"):
        candidate = source.parent / name.removeprefix("@loader_path/")
    elif name.startswith("@rpath/"):
        suffix = name.removeprefix("@rpath/")
        candidates = [directory / suffix for directory in search]
        # Resolve the original binary's own LC_RPATH entries before rewriting.
        for path in re.findall(r"\bpath (.+?) \(offset \d+\)", output("otool", "-l", str(source))):
            base = Path(path.replace("@loader_path", str(source.parent)))
            if base.is_absolute():
                candidates.append(base / suffix)
        candidate = next((p for p in candidates if p.is_file()), None)
    else:
        candidate = Path(name)
    if candidate is None or not candidate.is_file():
        raise RuntimeError(f"unresolved Mach-O dependency {name} of {source}")
    return candidate


core = register(args.core, libdir / args.core.name)
client = register(args.client, bindir / args.client.name)
compiler = register(args.compiler, bindir / args.compiler.name)
while queue:
    source, destination = queue.pop(0)
    if sys.platform == "darwin":
        dependencies = [line.strip().split(" (compatibility version", 1)[0]
                        for line in output("otool", "-L", str(source)).splitlines()[1:]]
        own_names = output("otool", "-D", str(source)).splitlines()[1:] if source.suffix == ".dylib" else []
        for name in dependencies:
            if name in own_names:
                continue
            dependency = resolve_macho(name, source)
            if dependency is None or dependency.resolve() == source:
                continue
            target = register(dependency, libdir / Path(name).name)
            relative = os.path.relpath(target, destination.parent)
            subprocess.run(["install_name_tool", "-change", name,
                            "@loader_path/" + relative, str(destination)], check=True)
        if destination.suffix == ".dylib":
            subprocess.run(["install_name_tool", "-id", "@rpath/" + destination.name,
                            str(destination)], check=True)
    else:
        names = re.findall(r"DLL Name:\s*(\S+)", output("objdump", "-p", str(source)))
        for name in names:
            candidates = [source.parent / name] + [directory / name for directory in search]
            dependency = next((p for p in candidates if p.is_file()), None)
            if dependency is None:
                system_file = Path(os.environ["SystemRoot"]) / "System32" / name
                if name.lower().startswith(("api-ms-", "ext-ms-")) or system_file.is_file():
                    system.add(name)
                    continue
                raise RuntimeError(f"unresolved PE dependency {name} of {source}")
            register(dependency, bindir / name)

shutil.copytree(args.xkb_dir, out / "share" / "X11" / "xkb", symlinks=False)
if sys.platform == "darwin":
    # install_name_tool changes invalidate Mach-O signatures on Apple silicon.
    for _, path in objects.values():
        subprocess.run(["codesign", "--force", "--sign", "-", "--timestamp=none", str(path)], check=True)
    launcher = out / "wayx.sh"
    launcher.write_text(f'''#!/bin/sh
set -eu
runtime=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export WAYX_XKB_DIR="$runtime/share/X11/xkb"
export WAYX_XKB_BIN_DIR="$runtime/bin"
exec "$runtime/bin/{client.name}" serve --core "$runtime/lib/{core.name}" "$@"
''')
    launcher.chmod(0o755)
else:
    (out / "wayx.ps1").write_text(f'''$ErrorActionPreference = 'Stop'
$env:WAYX_XKB_DIR = "$PSScriptRoot/share/X11/xkb"
$env:WAYX_XKB_BIN_DIR = "$PSScriptRoot/bin"
$env:PATH = "$PSScriptRoot/bin;" + $env:PATH
& "$PSScriptRoot/bin/{client.name}" serve --core "$PSScriptRoot/bin/{core.name}" @args
exit $LASTEXITCODE
''')
licenses = out / "licenses"
licenses.mkdir()
shutil.copy2("LICENSE", licenses / "WayX-LICENSE")
license_roots = set(path.resolve() for path in args.license_dir)
for source, _ in objects.values():
    if "/Cellar/" in source.as_posix():
        license_roots.add(source.parent.parent)
for index, root in enumerate(sorted(license_roots)):
    for path in root.rglob("*"):
        if path.is_file() and path.name.lower().startswith(("copying", "license", "copyright")):
            destination = licenses / f"{index}-{root.name}" / path.relative_to(root)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, destination)
manifest = {"abi": 2, "platform": sys.platform,
            "core_revision": "a8192aa70998a803155bdc2e73b7564a1415f33a",
            "core": str(core.relative_to(out)),
            "libraries": sorted(str(path.relative_to(out)) for _, path in objects.values()),
            "system_dependencies": sorted(system),
            "signing": "ad-hoc" if sys.platform == "darwin" else "unsigned"}
(out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(f"Staged {len(objects)} native binaries in {out}")
