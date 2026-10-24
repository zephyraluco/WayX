#!/usr/bin/env python3
"""Linux end-to-end: external OpenSSH -> isolated sshd -> WayX -> native pixels."""
import argparse
import os
from pathlib import Path
import pwd
import select
import shlex
import shutil
import socket
import struct
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--core', type=Path, required=True)
args = parser.parse_args()
binary, core = args.binary.resolve(), args.core.resolve()

def run(command, **kwargs):
    return subprocess.run(command, check=True, **kwargs)

def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]

def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

def wait_for(check, process):
    deadline = time.monotonic() + 15
    while not check():
        if process.poll() is not None:
            raise RuntimeError(f'Process exited during startup: {process.returncode}')
        if time.monotonic() >= deadline:
            raise RuntimeError('Startup timed out')
        time.sleep(.05)

def loopback_only(display):
    listeners = []
    for path in (Path('/proc/net/tcp'), Path('/proc/net/tcp6')):
        if not path.exists():
            continue
        for line in path.read_text().splitlines()[1:]:
            fields = line.split()
            address, port = fields[1].split(':')
            if int(port, 16) == 6000 + display and fields[3] == '0A':
                listeners.append(address)
    if not listeners or any(address not in ('0100007F', '00000000000000000000000001000000') for address in listeners):
        raise RuntimeError(f'X11 listener is not loopback-only: {listeners}')

with tempfile.TemporaryDirectory(prefix='wayx-openssh-') as directory:
    root = Path(directory)
    processes = []
    try:
        displays = [n for n in range(250, 300) if not Path(f'/tmp/.X11-unix/X{n}').exists()
                    and not Path(f'/tmp/.X{n}-lock').exists()][:2]
        if len(displays) != 2:
            raise RuntimeError('No free displays')
        host_display, display = displays
        host_env = {**os.environ, 'DISPLAY':f':{host_display}', 'RUST_LOG':'wayx=debug'}
        host_env.pop('XAUTHORITY', None)
        with (root/'xvfb.log').open('w') as log:
            host = subprocess.Popen(['Xvfb', f':{host_display}', '-screen', '0', '1280x1024x24', '-nolisten', 'tcp', '-ac'],
                                    stdout=log, stderr=log, env=host_env)
        processes.append(host)
        wait_for(lambda: Path(f'/tmp/.X11-unix/X{host_display}').exists(), host)
        flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'x11'], text=True))
        for name, source in [('client', 'idle-client'), ('observer', 'native-glx')]:
            run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror', f'tests/{source}.c', *flags, '-o', str(root/name)])
        for name in ('host-key', 'identity'):
            run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(root/name)])
        port = free_port()
        (root/'known_hosts').write_text(f'[127.0.0.1]:{port} '+(root/'host-key.pub').read_text())
        user = pwd.getpwuid(os.getuid()).pw_name
        config = root/'sshd_config'
        config.write_text(f'''Port {port}
ListenAddress 127.0.0.1
HostKey {root}/host-key
PidFile {root}/sshd.pid
AuthorizedKeysFile {root}/identity.pub
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
PermitRootLogin prohibit-password
X11Forwarding yes
X11UseLocalhost yes
X11DisplayOffset 400
XAuthLocation {shutil.which('xauth')}
SetEnv XAUTHORITY={root}/remote-Xauthority
LogLevel VERBOSE
''')
        if os.geteuid() == 0:
            Path('/run/sshd').mkdir(exist_ok=True)
        daemon_env = os.environ.copy()
        for key in ('DISPLAY', 'XAUTHORITY'):
            daemon_env.pop(key, None)
        with (root/'sshd.log').open('w') as log:
            daemon = subprocess.Popen([shutil.which('sshd') or '/usr/sbin/sshd', '-D', '-e', '-f', str(config)],
                                      stdout=log, stderr=log, env=daemon_env)
        processes.append(daemon)
        def listening():
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=.2): return True
            except OSError: return False
        wait_for(listening, daemon)
        for authenticated in (False, True):
            auth = root/'local-Xauthority'
            if authenticated:
                data = struct.pack('>H', 65535)
                for field in (b'', str(display).encode(), b'MIT-MAGIC-COOKIE-1', os.urandom(16)):
                    data += struct.pack('>H', len(field)) + field
                auth.write_bytes(data)
                auth.chmod(0o600)
            command = [str(binary), 'serve', '--rootless', '--core', str(core), '--', f':{display}',
                       '-listen', 'tcp', '-nolisten', 'local']
            if authenticated: command += ['-auth', str(auth)]
            with (root/f'wayx-{authenticated}.log').open('w') as log:
                server = subprocess.Popen(command, stdout=log, stderr=log, env=host_env)
            processes.append(server)
            local_env = {**host_env, 'DISPLAY':f'127.0.0.1:{display}', 'XAUTHORITY':str(auth)}
            def ready():
                return subprocess.run(['xdpyinfo'], env=local_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
            wait_for(ready, server)
            loopback_only(display)
            ssh = ['ssh', '-F', '/dev/null', '-Y', '-p', str(port), '-i', str(root/'identity'),
                   '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes', '-o', 'StrictHostKeyChecking=yes',
                   '-o', f'UserKnownHostsFile={root}/known_hosts', '-o', 'GlobalKnownHostsFile=/dev/null',
                   '-o', f'XAuthLocation={shutil.which("xauth")}', f'{user}@127.0.0.1', shlex.quote(str(root/'client'))]
            with (root/f'ssh-{authenticated}.log').open('w') as log:
                client = subprocess.Popen(ssh, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, env=local_env)
            processes.append(client)
            available, _, _ = select.select([client.stdout], [], [], 20)
            if not available or client.stdout.readline().strip() != b'DRAWN':
                raise RuntimeError('SSH-forwarded client did not draw')
            run([str(root/'observer'), 'WayX idle regression', 'solid'], env=host_env, timeout=15)
            client.communicate(input=b'\n', timeout=10)
            if client.returncode != 0:
                raise RuntimeError(f'SSH client failed: {client.returncode}')
            stop(server)
            if server.returncode != 0:
                raise RuntimeError(f'WayX shutdown failed: {server.returncode}')
            print(f'External OpenSSH: auth={authenticated}, native pixels and loopback-only listeners passed.', flush=True)
    except BaseException:
        subprocess.run(['xwininfo', '-root', '-tree'], env=host_env)
        for path in root.glob('*.log'):
            print(f'{path.name}:\n{path.read_text(errors="replace")}', flush=True)
        raise
    finally:
        for process in reversed(processes): stop(process)
