#!/usr/bin/env python3
"""Measure authenticated rootless idle CPU, including the internal WM/XIM connection."""
import argparse
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


parser = argparse.ArgumentParser()
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--core", type=Path, required=True)
parser.add_argument("--display", type=int, default=199)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix="wayx-idle-cpu-") as directory:
    auth = Path(directory) / "Xauthority"
    log = Path(directory) / "serve.log"
    subprocess.run([sys.executable, "scripts/create-xauthority.py", "--display", str(args.display),
                    "--output", str(auth)], check=True)
    env = os.environ.copy()
    env.update(RUST_LOG="wayx=debug", NO_COLOR="1")
    with log.open("w") as output:
        process = subprocess.Popen([str(args.binary.resolve()), "serve", "--rootless", "--core",
                                    str(args.core.resolve()), "--", f":{args.display}", "-screen", "0",
                                    "1280x800x24", "-crtcs", "1@1280x800", "-listen", "tcp",
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
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
