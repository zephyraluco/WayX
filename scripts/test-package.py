#!/usr/bin/env python3
"""Test relocated, verified artifacts without a checkout, Cargo, or C compiler."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
sys.dont_write_bytecode = True
from package_support import verify, digest

# Windows CI redirects these streams and otherwise uses a legacy code page.
# Test logs contain Unicode; replay them without changing or hiding failures.
sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--runtime", type=Path, required=True)
parser.add_argument("--test-kit", type=Path, required=True)
parser.add_argument("--expected-sha", required=True)
parser.add_argument("--logs", type=Path, required=True)
args = parser.parse_args()
runtime, kit, logs = args.runtime.resolve(), args.test_kit.resolve(), args.logs.resolve()
logs.mkdir(parents=True, exist_ok=True)
manifest = verify(runtime, args.expected_sha, "runtime")
tests = verify(kit, args.expected_sha, "test-kit")
if tests["runtime_manifest_sha256"] != digest(runtime/"manifest.json"):
    raise RuntimeError("Test kit was built for a different runtime archive")
if (manifest["abi"], manifest["target"]) != (tests["abi"], tests["target"]):
    raise RuntimeError("Runtime and test kit ABI/target differ")
(logs/"runtime-manifest.json").write_text(json.dumps(manifest, indent=2))
(logs/"test-kit-manifest.json").write_text(json.dumps(tests, indent=2))
# All subprocesses run inside the attachment; no Cargo.toml, C sources or build tree.
os.chdir(kit)
env = os.environ.copy()
env["PYTHONDONTWRITEBYTECODE"] = "1"
env["PYTHONIOENCODING"] = "utf-8"
for key in ("WAYX_XKB_DIR", "WAYX_XKB_BIN_DIR", "CARGO_TARGET_DIR", "PKG_CONFIG_PATH",
            "LIBRARY_PATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"):
    env.pop(key, None)
env.update(WAYX_XKB_DIR=str(runtime/"share/X11/xkb"), WAYX_XKB_BIN_DIR=str(runtime/"bin"),
           WAYX_TEST_CORE=str(runtime/manifest["core"]), WAYX_TEST_KIT=str(kit), NO_COLOR="1")
# A missing runtime dependency must not be rescued by development-library paths.
env["PATH"] = str(runtime/"bin")+os.pathsep+env["PATH"]
if sys.platform == "linux":
    env.update(LD_LIBRARY_PATH=str(runtime/"lib"), WGPU_BACKEND="vulkan")
    cache = logs/"xdg"; cache.mkdir(mode=0o700, exist_ok=True)
    env["XDG_RUNTIME_DIR"] = str(cache)
if sys.platform == "darwin":
    # Xlib is a test client dependency, not a WayX runtime requirement.
    prefix = subprocess.check_output(["brew", "--prefix", "libx11"], text=True).strip()
    env["XLOCALEDIR"] = prefix+"/share/X11/locale"
    modules = Path(prefix)/"lib/X11/locale"
    if modules.is_dir():
        destination = logs/"xlib-locales"
        shutil.copytree(modules, destination)
        for module in (destination/"common").glob("*.2.so"):
            module.with_name(module.name.removesuffix(".2.so")+".so.2").symlink_to(module.name)
        env["XLOCALELIBDIR"] = str(destination)


def executable(name):
    relative = tests["executables"].get(name)
    if not relative: raise RuntimeError("Missing precompiled test: "+name)
    return str(kit/relative)


def run(name, command, extra=None, timeout=180):
    print(f"Running {name}", flush=True)
    selected = env.copy(); selected.update(extra or {})
    with (logs/(name+".log")).open("w", encoding="utf-8") as output:
        try:
            subprocess.run([str(item) for item in command], env=selected, stdout=output,
                           stderr=subprocess.STDOUT, check=True, timeout=timeout)
        except BaseException:
            print((logs/(name+".log")).read_text(encoding="utf-8", errors="replace"), flush=True)
            raise
    print((logs/(name+".log")).read_text(encoding="utf-8", errors="replace"), flush=True)


run("package-support", [sys.executable, "scripts/test-package-support.py"])
for name in tests["units"]:
    run(name, [executable(name), "--nocapture"])
if "c-bridge" in tests["executables"]: run("c-bridge", [executable("c-bridge")])
for name in ("xlibre", "rootless", "window_manager", "desktop_layout"):
    run(name, [executable(name), "--ignored", "--nocapture", "--test-threads=1"])
run("local-display-no-auth", [executable("local_display"), "--ignored", "--nocapture"])
run("local-display-auth", [executable("local_display"), "--ignored", "--nocapture"], {"WAYX_TEST_LOCAL_AUTH":"1"})
run("multi-output", [executable("xlibre"), "--ignored", "--nocapture"], {"WAYX_TEST_MONITORS":"1"})
if sys.platform != "win32":
    run("xim", [executable("ime"), "--ignored", "--nocapture"], {"WAYX_TEST_XIM_CLIENT":executable("xim-client")})
if sys.platform == "linux":
    ogre = executable("ogre-plugin")
    for name in ("glx-client", "ogre-client"):
        for rootless in (False, True):
            gl_env = {"WAYX_TEST_GLX_CLIENT":executable(name), "WAYX_TEST_OGRE_PLUGIN":str(ogre)}
            if rootless: gl_env["WAYX_TEST_ROOTLESS"]="1"
            run(f"{name}-rootless-{rootless}", [executable("glx"), "--ignored", "--nocapture"], gl_env)
client, core = runtime/manifest["client"], runtime/manifest["core"]
if sys.platform == "darwin":
    run("rootless-lifecycle", [sys.executable, "scripts/test-macos-lifecycle.py", "--binary", client,
                              "--core", core, "--client", executable("idle-client")])
if sys.platform == "linux":
    run("gpu-readback", [client, "gpu-smoke"])
    run("native", [sys.executable, "scripts/test-native.py", "--binary", client, "--core", core,
                   "--glx-client", executable("glx-client"), "--gpu-cycles", "--test-kit", kit], timeout=300)
    run("native-geometry", ["xvfb-run", "-a", sys.executable, "scripts/test-idle-cpu.py", "--binary", client,
                            "--core", core, "--geometry", "--test-kit", kit])
else:
    run("native", [client, "serve", "--core", core, "--frames", "3", "--", ":196", "-screen", "0", "640x480x24",
                   "-crtcs", "1@640x480", "-listen" if sys.platform=="win32" else "-nolisten", "tcp", "-ac"])
    if sys.platform == "win32":
        run("native-geometry-gpu", [sys.executable, "scripts/test-idle-cpu.py", "--binary", client,
                                   "--core", core, "--geometry", "--gpu-cycles", "--test-kit", kit], timeout=300)
# Exercise the actual relocatable entry point, not only explicit --core invocations.
launcher = runtime/manifest["launcher"]
command = [str(launcher), "--frames", "3", "--", ":196", "-screen", "0", "640x480x24", "-crtcs", "1@640x480",
           "-listen" if sys.platform=="win32" else "-nolisten", "tcp", "-ac"]
if sys.platform == "win32": command = ["cmd.exe", "/d", "/c", *command]
if sys.platform == "linux": command = ["xvfb-run", "-a", *command]
run("relocated-launcher", command)
(logs/"result.json").write_text(json.dumps({"source_sha":args.expected_sha, "platform":sys.platform, "result":"passed"})+"\n")
print("Verified package and precompiled test-kit checks passed.")
