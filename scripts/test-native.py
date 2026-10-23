#!/usr/bin/env python3
"""Linux native-window smoke tests. Start and clean up only our isolated servers."""
import argparse
import os
from pathlib import Path
import shutil
import shlex
import select
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument("--binary", type=Path, default=Path("target/debug/wayx"))
parser.add_argument("--core", type=Path)
parser.add_argument("--glx-client", type=Path)
parser.add_argument("--gpu-cycles", action="store_true")
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

def idle_cpu(process, seconds=2, limit=.05):
    def cpu_time():
        fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    before = cpu_time()
    time.sleep(seconds)
    if process.poll() is not None:
        raise RuntimeError("Idle server exited during CPU sampling")
    elapsed = cpu_time() - before
    print(f"Idle CPU: {elapsed * 1000:.1f} ms over {seconds}s", flush=True)
    if elapsed > limit:
        raise RuntimeError(f"Idle CPU time {elapsed:.3f}s exceeds {limit:.3f}s budget")

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
                resize_client = Path(temp) / "resize-client"
                subprocess.run(["cc", "-std=c11", "-D_POSIX_C_SOURCE=200809L", "-Wall", "-Wextra", "-Werror",
                                "tests/native-resize.c", *flags, "-o", str(resize_client)], check=True)
                desktop = subprocess.Popen([binary, "serve", "--core", str(args.core.resolve()),
                                            "--", f":{core_display}", "-screen", "0", "1280x800x24",
                                            "-crtcs", "1@1280x800", "-nolisten", "tcp", "-auth", str(auth)], env=env)
                try:
                    resize_env = env.copy()
                    resize_env["XAUTHORITY"] = str(auth)
                    subprocess.run([str(resize_client), f":{core_display}", f":{host}"], env=resize_env, check=True, timeout=30)
                finally:
                    desktop.terminate()
                    try:
                        if desktop.wait(timeout=5) != 0:
                            raise RuntimeError("Resized desktop shutdown failed")
                    except subprocess.TimeoutExpired:
                        desktop.kill()
                        desktop.wait()
                        raise
                client = Path(temp) / "rootless-client"
                subprocess.run(["cc", "-std=c11", "-D_POSIX_C_SOURCE=200809L", "-Wall", "-Wextra", "-Werror",
                                "tests/native-rootless.c", *flags, "-o", str(client)], check=True)
                rootless = subprocess.Popen([binary, "serve", "--rootless", "--core", str(args.core.resolve()),
                                             "--", f":{core_display}",
                                             "-nolisten", "tcp", "-auth", str(auth)], env=env)
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
            if args.core:
                idle_client = Path(temp) / "idle-client"
                subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "tests/idle-client.c", *flags, "-o", str(idle_client)], check=True)
                for rootless_mode in (True, False):
                    idle_log = Path(temp) / f"idle-{rootless_mode}.log"
                    idle_env = env.copy()
                    idle_env.update(RUST_LOG="wayx=debug", NO_COLOR="1")
                    with idle_log.open("w") as output:
                        command = [binary, "serve", "--core", str(args.core.resolve())]
                        if rootless_mode:
                            command.append("--rootless")
                        command += ["--", f":{core_display}"]
                        if not rootless_mode:
                            command += ["-screen", "0", "1280x800x24", "-crtcs", "1@1280x800"]
                        command += ["-nolisten", "tcp", "-auth", str(auth)]
                        idle = subprocess.Popen(command, env=idle_env, stdout=output, stderr=output)
                        client = None
                        try:
                            for _ in range(100):
                                if idle.poll() is not None:
                                    raise RuntimeError(idle_log.read_text())
                                if Path(f"/tmp/.X11-unix/X{core_display}").exists():
                                    break
                                time.sleep(.05)
                            else:
                                raise RuntimeError("Idle server readiness timeout")
                            # Includes -auth: this enables the internal WM/XIM/selection connection.
                            time.sleep(1)
                            idle_cpu(idle)
                            client_env = idle_env.copy()
                            client_env["XAUTHORITY"] = str(auth)
                            client = subprocess.Popen([str(idle_client), f":{core_display}"], env=client_env, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
                            def drawn():
                                if not select.select([client.stdout], [], [], 5)[0] or client.stdout.readline() != b"DRAWN\n":
                                    raise RuntimeError("Idle test client did not draw")
                            drawn()
                            time.sleep(1)
                            before = idle_log.read_text().count("frame presented")
                            if before == 0:
                                raise RuntimeError("Idle test never presented a frame")
                            idle_cpu(idle)
                            if idle_log.read_text().count("frame presented") != before:
                                raise RuntimeError("Static window kept presenting frames")
                            client.stdin.write(b"d")
                            client.stdin.flush()
                            drawn()
                            for _ in range(100):
                                if idle_log.read_text().count("frame presented") > before:
                                    break
                                time.sleep(.05)
                            else:
                                raise RuntimeError("Damage after idle did not wake presentation")
                            client.communicate(input=b"q", timeout=5)
                            if client.returncode != 0:
                                raise RuntimeError("Idle test client failed")
                        finally:
                            if client is not None and client.poll() is None:
                                client.kill(); client.wait()
                            idle.terminate()
                            try:
                                code = idle.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                idle.kill(); idle.wait()
                                raise RuntimeError("Idle server shutdown hung")
                            if code != 0:
                                raise RuntimeError(f"Idle server failed: {idle_log.read_text()}")
                    print(f"Idle rootless={rootless_mode}: zero extra presents; damage woke rendering.")
            if args.core:
                conflict = subprocess.run([binary, "serve", "--rootless", "--core", str(args.core.resolve()),
                                           "--", f":{core_display}", "-screen", "0", "64x64x24", "-crtcs", "1@64x64", "-nolisten", "tcp", "-ac"],
                                          env=env, capture_output=True, text=True, timeout=5)
                if conflict.returncode == 0 or "conflict with the host desktop" not in conflict.stdout + conflict.stderr:
                    raise RuntimeError("Rootless conflicting screen options did not fail clearly: " + conflict.stdout + conflict.stderr)
            if args.gpu_cycles and args.core:
                subprocess.run([sys.executable, "scripts/test-idle-cpu.py", "--binary", binary,
                                "--core", str(args.core.resolve()), "--display", str(core_display), "--gpu-cycles"],
                               env=env, check=True, timeout=180)
            failed = subprocess.run([binary, "serve", "--rootless", "--core", str(Path(temp) / "missing-core")], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
            if failed.returncode == 0:
                raise RuntimeError("Missing core did not fail")
            print("Native window rendering and requested XLibre lifecycle checks passed.")
        finally:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
