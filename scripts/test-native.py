#!/usr/bin/env python3
"""Linux native-window smoke tests. Start and clean up only our isolated servers."""
import argparse
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("--binary", type=Path, default=Path("target/debug/wayx"))
parser.add_argument("--core", type=Path)
parser.add_argument("--glx-client", type=Path)
args = parser.parse_args()
binary = str(args.binary.resolve())
xvfb = os.environ.get("WAYX_TEST_XVFB") or shutil.which("Xvfb")
if not xvfb:
    parser.error("install Xvfb or set WAYX_TEST_XVFB")
# Avoid displays already in use. A startup race remains an explicit failure.
displays = [i for i in range(200, 250) if not Path(f"/tmp/.X11-unix/X{i}").exists()
            and not Path(f"/tmp/.X{i}-lock").exists()][:2]
if len(displays) < 2:
    parser.error("no free test display numbers")
host, core_display = displays
with tempfile.TemporaryDirectory(prefix="wayx-native-") as temp:
    env = os.environ.copy()
    env.update(DISPLAY=f":{host}", WINIT_UNIX_BACKEND="x11", XDG_RUNTIME_DIR=temp)
    with open(Path(temp) / "xvfb.log", "w+") as log:
        server = subprocess.Popen([xvfb, f":{host}", "-screen", "0", "1280x1024x24",
                                   "-nolisten", "tcp", "-ac"], stdout=log, stderr=log)
        try:
            for _ in range(100):
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read())
                if Path(f"/tmp/.X11-unix/X{host}").exists():
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Xvfb readiness timeout")
            subprocess.run([binary, "demo", "--frames", "3"], env=env, check=True, timeout=30)
            if args.core:
                auth = Path(temp) / "Xauthority"
                subprocess.run(["python3", "scripts/create-xauthority.py", "--display",
                                str(core_display), "--output", str(auth)], check=True)
                subprocess.run([binary, "serve", "--core", str(args.core.resolve()), "--frames", "3",
                                "--", f":{core_display}", "-screen", "0", "1280x800x24",
                                "-crtcs", "1@1280x800", "-nolisten", "tcp", "-auth", str(auth)],
                               env=env, check=True, timeout=30)
                if Path(f"/tmp/.X11-unix/X{core_display}").exists():
                    raise RuntimeError("XLibre socket survived normal shutdown")
                flags = shlex.split(subprocess.check_output(["pkg-config", "--cflags", "--libs", "x11"], text=True))
                client = Path(temp) / "rootless-client"
                subprocess.run(["cc", "-std=c11", "-D_POSIX_C_SOURCE=200809L", "-Wall", "-Wextra", "-Werror",
                                "tests/native-rootless.c", *flags, "-o", str(client)], check=True)
                rootless = subprocess.Popen([binary, "serve", "--rootless", "--core", str(args.core.resolve()),
                                             "--", f":{core_display}", "-screen", "0", "1280x800x24",
                                             "-crtcs", "1@1280x800", "-nolisten", "tcp", "-extension", "MIT-SHM", "-auth", str(auth)], env=env)
                try:
                    client_env = env.copy()
                    client_env["XAUTHORITY"] = str(auth)
                    subprocess.run([str(client), f":{core_display}", f":{host}"], env=client_env, check=True, timeout=30)
                    if args.glx_client:
                        observer = Path(temp) / "native-glx"
                        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                                        "tests/native-glx.c", *flags, "-o", str(observer)], check=True)
                        gl_env = client_env.copy()
                        gl_env.update(DISPLAY=f":{core_display}", LIBGL_ALWAYS_SOFTWARE="1", GALLIUM_DRIVER="llvmpipe")
                        gl_env.pop("LIBGL_ALWAYS_INDIRECT", None)
                        gl_client = subprocess.Popen([str(args.glx_client.resolve())], env=gl_env, stdin=subprocess.PIPE)
                        try:
                            subprocess.run([str(observer)], env=env, check=True, timeout=20)
                            gl_client.communicate(input=b"\n", timeout=5)
                            if gl_client.returncode != 0:
                                raise RuntimeError(f"GLX client failed: {gl_client.returncode}")
                        finally:
                            if gl_client.poll() is None:
                                gl_client.kill()
                                gl_client.wait()
                    if rootless.poll() is not None:
                        raise RuntimeError("Closing a rootless window terminated the server")
                finally:
                    rootless.terminate()
                    try:
                        code = rootless.wait(timeout=5)
                        if code != 0:
                            raise RuntimeError(f"Rootless shutdown failed with exit status {code}")
                    except subprocess.TimeoutExpired:
                        rootless.kill()
                        rootless.wait()
            print("Native window rendering and requested XLibre lifecycle checks passed.")
        finally:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
