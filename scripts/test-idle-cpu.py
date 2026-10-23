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


def memory_usage(process):
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes
        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD)] + [
                (name, ctypes.c_size_t) for name in ["PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage",
                "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage", "PrivateUsage"]]
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        psapi.GetProcessMemoryInfo.restype = wintypes.BOOL
        handle = kernel.OpenProcess(0x410, False, process.pid)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            counters = Counters(); counters.cb = ctypes.sizeof(counters)
            if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                raise ctypes.WinError(ctypes.get_last_error())
            return counters.WorkingSetSize, counters.PrivateUsage
        finally:
            kernel.CloseHandle(handle)
    status = Path(f"/proc/{process.pid}/status").read_text()
    rss = int(next(line.split()[1] for line in status.splitlines() if line.startswith("VmRSS:"))) * 1024
    rollup = Path(f"/proc/{process.pid}/smaps_rollup").read_text()
    private = sum(int(line.split()[1]) * 1024 for line in rollup.splitlines() if line.startswith(("Private_Clean:", "Private_Dirty:")))
    return rss, private


def build_fixture(name):
    command = ["cargo", "test", "--locked", "--test", name, "--no-run", "--message-format=json"]
    if sys.platform == "win32":
        command.append("--release")
    built = subprocess.run(command, capture_output=True, text=True)
    if built.returncode != 0:
        raise RuntimeError(built.stdout + built.stderr)
    for line in built.stdout.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == name and item.get("executable"):
            return Path(item["executable"])
    raise RuntimeError(f"No {name} test executable")


def gpu_cycles(binary, process, display, auth, log):
    import re
    env = os.environ.copy()
    env.update(WAYX_GEOMETRY_DISPLAY=str(display), WAYX_GEOMETRY_AUTH=str(auth))
    client = subprocess.Popen([str(binary), "--ignored", "--nocapture"], env=env,
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    lines = queue.Queue()
    def read_lines():
        for line in client.stdout:
            lines.put(line.rstrip())
        lines.put(None)
    reader = threading.Thread(target=read_lines, daemon=True); reader.start()
    def expect(message):
        deadline = time.monotonic() + 20
        received = []
        while time.monotonic() < deadline:
            line = lines.get(timeout=max(.01, deadline-time.monotonic()))
            if line is None:
                raise RuntimeError(f"GPU client exited before {message}: {received}")
            received.append(line)
            if line == message:
                return received
        raise RuntimeError(f"GPU client timeout: {received}")
    def command(text):
        start = len(log.read_text())
        client.stdin.write(text + "\n"); client.stdin.flush()
        output = expect("GPU DONE")
        return start, output
    def resources(windows, start=0, retired=False):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            for line in reversed(log.read_text()[start:].splitlines()):
                if "GPU resource state" not in line or (retired and 'phase="retired"' not in line):
                    continue
                values = {name: int(value) for name, value in re.findall(r"\b(windows|surfaces|devices|adapters|textures|bindings|pipelines)=(\d+)", line)}
                if values.get("windows") != windows:
                    continue
                if values.get("devices") != 1 or values.get("adapters") != 1 or values.get("surfaces") != windows or values.get("textures") != windows or values.get("bindings") != windows or values.get("pipelines") != 1:
                    raise RuntimeError(f"Unexpected GPU lifetime resources: {line}")
                return
            if process.poll() is not None:
                raise RuntimeError("GPU server exited: " + log.read_text())
            time.sleep(.02)
        raise RuntimeError(f"No GPU resource report for {windows} windows")
    def opened(width, height, count=1):
        start, output = command(f"open {width} {height} {count}")
        ids = next(line[len("GPU OPEN "):].split(",") for line in output if line.startswith("GPU OPEN "))
        resources(count, start)
        deadline = time.monotonic()+15
        while not all(f"frame presented window={identifier}" in log.read_text()[start:] for identifier in ids):
            if time.monotonic() >= deadline:
                raise RuntimeError("GPU lifecycle windows were not presented")
            time.sleep(.02)
    def closed():
        start, _ = command("close")
        resources(0, start, retired=True)
        time.sleep(.1)
        return memory_usage(process)
    try:
        expect("GPU READY")
        opened(200, 200, 2)
        command("resize 400 320")
        start, _ = command("close-one")
        resources(1, start, retired=True)
        closed()
        for width, height in [(200, 200), (1600, 1000)]:
            for _ in range(4):
                opened(width, height); closed()
            samples = []
            for cycle in range(12):
                opened(width, height)
                rss, private = closed(); samples.append(private)
                print(f"GPU cycle {width}x{height} {cycle+1}: RSS={rss/2**20:.1f} MiB private={private/2**20:.1f} MiB", flush=True)
            growth = sum(samples[-3:])/3-sum(samples[:3])/3
            if growth > 32*2**20:
                raise RuntimeError(f"Post-warmup GPU memory growth {growth/2**20:.1f} MiB exceeds 32 MiB budget")
        contents = log.read_text()
        if contents.count("shared GPU device initialized") != 1 or contents.count("render adapter") != 1:
            raise RuntimeError("Native windows recreated the shared GPU device/adapter")
        client.stdin.write("quit\n"); client.stdin.flush(); client.stdin.close()
        if client.wait(timeout=5) != 0:
            raise RuntimeError("GPU lifecycle client failed")
        print("Shared GPU lifetime passed: one device, isolated multi-window close, zero live window resources after retirement, bounded memory growth.", flush=True)
    finally:
        if client.poll() is None:
            client.kill(); client.wait()
        reader.join(timeout=2)


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
        return (contents.count("applying server position"),contents.count("WayX host desktop layout applied:"),contents.count("applying native size hints"))
    def verify_drag(before, steps=300):
        after=corrections()
        count = after[0] - before[0]
        if after[1] != before[1]:
            raise RuntimeError("Dragging changed the host desktop layout")
        if after[2] != before[2]:
            raise RuntimeError("Dragging reapplied native size hints")
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
parser.add_argument("--gpu-cycles", action="store_true")
args = parser.parse_args()
fixture = build_fixture("native_geometry") if args.geometry else None
gpu_fixture = build_fixture("native_gpu") if args.gpu_cycles else None
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
            if gpu_fixture is not None:
                gpu_cycles(gpu_fixture, process, args.display, auth, log)
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
