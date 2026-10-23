#!/usr/bin/env python3
"""Measure authenticated rootless idle CPU, including the internal WM/XIM connection."""
import argparse
import json
import queue
import threading
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def cpu_time(process):
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        kernel.GetProcessTimes.restype = wintypes.BOOL
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        handle = kernel.OpenProcess(0x1000, False, process.pid)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            created, exited, system, user = (wintypes.FILETIME() for _ in range(4))
            if not kernel.GetProcessTimes(handle, ctypes.byref(created), ctypes.byref(exited),
                                          ctypes.byref(system), ctypes.byref(user)):
                raise ctypes.WinError(ctypes.get_last_error())
            return sum((value.dwHighDateTime << 32) | value.dwLowDateTime
                       for value in (system, user)) / 10_000_000
        finally:
            kernel.CloseHandle(handle)
    if sys.platform.startswith("linux"):
        fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    raise RuntimeError("CPU sampling supports Windows and Linux")


def geometry_client(binary, process, display, auth, log):
    env = os.environ.copy()
    env.update(WAYX_GEOMETRY_DISPLAY=str(display), WAYX_GEOMETRY_AUTH=str(auth))
    client = subprocess.Popen([str(binary), "--ignored", "--nocapture"], env=env,
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    lines = queue.Queue()
    def read_lines():
        for line in client.stdout:
            lines.put(line.rstrip())
        lines.put(None)
    reader = threading.Thread(target=read_lines, daemon=True)
    reader.start()
    def expect(message):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            line = lines.get(timeout=max(.01, deadline-time.monotonic()))
            if line is None:
                raise RuntimeError("Geometry client exited before " + message)
            print(line, flush=True)
            if line == message:
                return
        raise RuntimeError("Geometry client timeout")
    def command(text):
        client.stdin.write(text + "\n"); client.stdin.flush()
        expect("GEOMETRY DONE")
    def corrections():
        contents=log.read_text()
        return (contents.count("applying server position"),contents.count("WayX host desktop layout applied:"))
    def verify_drag(before, steps=300):
        after=corrections()
        count = after[0] - before[0]
        if after[1] != before[1]:
            raise RuntimeError("Dragging changed the host desktop layout")
        if count:
            raise RuntimeError(f"Accepted local drag caused {count} native position corrections")
        print(f"{steps}-step drag: zero native position corrections.", flush=True)
    try:
        expect("GEOMETRY READY")
        time.sleep(1)
        command("check 400 287 5000")
        if sys.platform == "win32":
            import ctypes
            from ctypes import wintypes
            user = ctypes.WinDLL("user32", use_last_error=True)
            user.SetProcessDPIAware()
            callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
            user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
            user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
            user.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
            user.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
            user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
            user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
            user.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
            found = []
            @callback_type
            def visit(hwnd, _):
                pid = wintypes.DWORD()
                user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                title = ctypes.create_unicode_buffer(256)
                user.GetWindowTextW(hwnd, title, 256)
                if pid.value == process.pid and title.value == "WayX geometry regression":
                    found.append(hwnd)
                return True
            user.EnumWindows(visit, 0)
            if len(found) != 1:
                raise RuntimeError("Could not find unique native geometry window")
            hwnd = found[0]
            user.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
            def native_position():
                point = wintypes.POINT(0, 0)
                if not user.ClientToScreen(hwnd, ctypes.byref(point)):
                    raise ctypes.WinError(ctypes.get_last_error())
                return point.x, point.y
            def verify_position(x, y):
                if native_position() != (x, y):
                    raise RuntimeError(f"Native position {native_position()} differs from X11 {x},{y}")
            def move(x, y):
                rect = wintypes.RECT()
                if not user.GetWindowRect(hwnd, ctypes.byref(rect)):
                    raise ctypes.WinError(ctypes.get_last_error())
                client = native_position()
                if not user.SetWindowPos(hwnd, None, x-(client[0]-rect.left), y-(client[1]-rect.top), 0, 0, 0x15):
                    raise ctypes.WinError(ctypes.get_last_error())
            def native_size():
                rect = wintypes.RECT()
                if not user.GetClientRect(hwnd, ctypes.byref(rect)):
                    raise ctypes.WinError(ctypes.get_last_error())
                return rect.right-rect.left, rect.bottom-rect.top
            def verify_size(width, height):
                if native_size() != (width, height):
                    raise RuntimeError(f"Native size {native_size()} differs from X11 {width}x{height}")
            def resize(width, height):
                outer = wintypes.RECT()
                if not user.GetWindowRect(hwnd, ctypes.byref(outer)):
                    raise ctypes.WinError(ctypes.get_last_error())
                current = native_size()
                if not user.SetWindowPos(hwnd, None, 0, 0, width+outer.right-outer.left-current[0], height+outer.bottom-outer.top-current[1], 0x16):
                    raise ctypes.WinError(ctypes.get_last_error())
            command("position 20 20 1000"); verify_position(20, 20)
            before = corrections()
            for index in range(400):
                move(100 + index, 120)
            command("position 499 120 1000"); verify_position(499, 120)
            verify_drag(before, 400)
            command("paint on")
            before = corrections()
            for index in range(300):
                move(500 + index, 120)
                time.sleep(.003)
            command("position 799 120 1000"); verify_position(799, 120)
            command("paint off")
            verify_drag(before)
            command("remote-position -120 80"); verify_position(-120, 80)
            command("border 3")
            command("remote-position 140 160"); verify_position(140, 160)
            move(220, 240)
            command("position 220 240 1000"); verify_position(220, 240)
            command("border 0")
            verify_size(400, 287)
            resize(720, 510)
            command("check 720 510 1000"); verify_size(720, 510)
            for width, height in [(800, 600), (640, 480), (760, 520)]:
                resize(width, height)
            command("check 760 520 1000"); verify_size(760, 520)
            command("hints")
            time.sleep(.2)
            resize(703, 497)
            command("check 696 490 1000"); verify_size(696, 490)
            command("remote 512 310"); verify_size(512, 310)
            user.ShowWindow(hwnd, 6); time.sleep(.2)
            user.ShowWindow(hwnd, 9)
            command("check 512 310 1000"); verify_size(512, 310)
            command("position 220 240 1000"); verify_position(220, 240)
        else:
            command("position 20 20 1000")
            before = corrections()
            command("local-position 499 120")
            verify_drag(before, 400)
            command("paint on")
            before = corrections()
            command("drag-position 799 120")
            command("paint off")
            verify_drag(before)
            command("remote-position -120 80")
            command("border 3")
            command("remote-position 140 160")
            command("local-position 220 240")
            command("border 0")
            command("remote 640 480")
            command("remote 400 287")
        client.stdin.write("quit\n"); client.stdin.flush(); client.stdin.close()
        if client.wait(timeout=5) != 0:
            raise RuntimeError("Geometry client failed")
        print("Passive geometry, resize convergence and native/X11 agreement passed.", flush=True)
    finally:
        if client.poll() is None:
            client.kill(); client.wait()
        reader.join(timeout=2)


parser = argparse.ArgumentParser()
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--core", type=Path, required=True)
parser.add_argument("--display", type=int, default=199)
parser.add_argument("--geometry", action="store_true")
args = parser.parse_args()
fixture = None
if args.geometry:
    build_command = ["cargo", "test", "--locked", "--test", "native_geometry", "--no-run", "--message-format=json"]
    if sys.platform == "win32":
        build_command.append("--release")  # Reuse the native CI release dependencies.
    built = subprocess.run(build_command, capture_output=True, text=True)
    if built.returncode != 0:
        raise RuntimeError(built.stdout + built.stderr)
    for line in built.stdout.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "native_geometry" and item.get("executable"):
            fixture = Path(item["executable"])
    if fixture is None:
        raise RuntimeError("No native geometry test executable")
with tempfile.TemporaryDirectory(prefix="wayx-idle-cpu-") as directory:
    auth = Path(directory) / "Xauthority"
    log = Path(directory) / "serve.log"
    subprocess.run([sys.executable, "scripts/create-xauthority.py", "--display", str(args.display),
                    "--output", str(auth)], check=True)
    env = os.environ.copy()
    env.update(RUST_LOG="wayx=debug", NO_COLOR="1")
    with log.open("w") as output:
        process = subprocess.Popen([str(args.binary.resolve()), "serve", "--rootless", "--core",
                                    str(args.core.resolve()), "--", f":{args.display}", "-listen", "tcp",
                                    "-nolisten", "local", "-auth", str(auth)],
                                   env=env, stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + 15
            while "internal X11 bridge ready" not in log.read_text():
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError(f"Internal authenticated bridge did not start: {log.read_text()}")
                time.sleep(.05)
            time.sleep(2)
            for _ in range(2):
                before = cpu_time(process)
                time.sleep(2)
                if process.poll() is not None:
                    raise RuntimeError(log.read_text())
                elapsed = cpu_time(process) - before
                print(f"Authenticated rootless idle CPU: {elapsed * 1000:.1f} ms / 2s", flush=True)
                if elapsed > .05:
                    raise RuntimeError(f"Idle CPU time {elapsed:.3f}s exceeds 50 ms budget")
            if fixture is not None:
                geometry_client(fixture, process, args.display, auth, log)
        except Exception:
            print(log.read_text(), flush=True)
            raise
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
