#!/usr/bin/env python3
"""Native macOS: verify real AppKit policy transitions while XLibre stays alive."""
import argparse
import os
from pathlib import Path
import select
import socket
import struct
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--core', type=Path, required=True)
parser.add_argument('--client', type=Path, required=True)
args = parser.parse_args()
if sys.platform != 'darwin':
    parser.error('Requires a native macOS GUI session')

def x11_ready(display):
    try:
        with socket.create_connection(('127.0.0.1', 6000 + display), timeout=.5) as stream:
            stream.sendall(b'l\0' + struct.pack('<5H', 11, 0, 0, 0, 0))
            return stream.recv(8)[:1] == b'\x01'
    except OSError:
        return False

display = None
for candidate in range(270, 300):
    if Path(f'/tmp/.X{candidate}-lock').exists():
        continue
    try:
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 6000 + candidate))
    except OSError:
        continue
    display = candidate
    break
if display is None:
    raise RuntimeError('No free lifecycle test display')

with tempfile.TemporaryDirectory(prefix='wayx-macos-lifecycle-') as directory:
    log = Path(directory)/'server.log'
    children = []
    env = {**os.environ, 'RUST_LOG':'wayx=debug', 'NO_COLOR':'1'}
    with log.open('w') as output:
        server = subprocess.Popen([str(args.binary.resolve()), 'serve', '--rootless', '--core', str(args.core.resolve()),
                                   '--', f':{display}', '-listen', 'tcp', '-nolisten', 'local'],
                                  stdout=output, stderr=subprocess.STDOUT, env=env)
    def transitions():
        return [line for line in log.read_text(errors='replace').splitlines()
                if 'macOS application presentation' in line]
    def wait(check):
        deadline = time.monotonic() + 20
        while not check():
            if server.poll() is not None:
                raise RuntimeError(f'WayX exited during lifecycle check: {server.returncode}')
            if time.monotonic() >= deadline:
                raise RuntimeError('Lifecycle check timed out')
            time.sleep(.05)
    def expect(count, visible):
        def check():
            lines = transitions()
            if len(lines) > count:
                raise RuntimeError(f'Unexpected activation transition: {lines}')
            return len(lines) == count and f'visible={str(visible).lower()}' in lines[-1]
        wait(check)
    def open_client():
        client_env = {**env, 'DISPLAY':f'127.0.0.1:{display}'}
        client_env.pop('XAUTHORITY', None)
        client = subprocess.Popen([str(args.client.resolve())], env=client_env, stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        children.append(client)
        if not select.select([client.stdout], [], [], 15)[0] or client.stdout.readline().strip() != b'DRAWN':
            raise RuntimeError('X11 client did not map and draw')
        return client
    def close_client(client):
        _, error = client.communicate(input=b'\n', timeout=10)
        if client.returncode:
            raise RuntimeError(f'Client failed: {error.decode(errors="replace")}')
    try:
        wait(lambda: x11_ready(display))
        expect(1, False)
        first = open_client()
        expect(2, True)
        second = open_client()
        close_client(first)
        time.sleep(.4)
        expect(2, True)
        close_client(second)
        expect(3, False)
        if not x11_ready(display):
            raise RuntimeError('Hidden service stopped accepting X11 clients')
        third = open_client()
        expect(4, True)
        close_client(third)
        expect(5, False)
        print('Native AppKit policy: hidden -> visible -> hidden -> visible -> hidden; XLibre remains alive.')
    except BaseException:
        print(log.read_text(errors='replace'), flush=True)
        raise
    finally:
        for process in [*reversed(children), server]:
            if process.poll() is None:
                process.terminate()
                try: process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
