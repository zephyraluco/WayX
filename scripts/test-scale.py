#!/usr/bin/env python3
"""Linux native scaling checks; each server uses an independent XLibre process."""
import argparse
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time
from test_fixtures import prebuilt

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary',type=Path,required=True)
parser.add_argument('--core',type=Path,required=True)
args=parser.parse_args()
binary,core=args.binary.resolve(),args.core.resolve()

def stop(process):
    if process.poll() is None:
        process.terminate()
        try:process.wait(timeout=5)
        except subprocess.TimeoutExpired:process.kill();process.wait()

with tempfile.TemporaryDirectory(prefix='wayx-scale-') as directory:
    root=Path(directory)
    client=prebuilt('scale-client')
    if client is None:
        client=root/'scale-client'
        flags=shlex.split(subprocess.check_output(['pkg-config','--cflags','--libs','x11'],text=True))
        subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','tests/scale-client.c',*flags,'-lm','-l:libXtst.so.6','-o',str(client)],check=True)
    displays=[n for n in range(300,350) if not Path(f'/tmp/.X{n}-lock').exists() and not Path(f'/tmp/.X11-unix/X{n}').exists()][:2]
    if len(displays)!=2:raise RuntimeError('No free test displays')
    host,display=displays
    env={**os.environ,'DISPLAY':f':{host}','WINIT_UNIX_BACKEND':'x11'}
    env.pop('XAUTHORITY',None)
    with (root/'xvfb.log').open('w') as output:
        xvfb=subprocess.Popen(['Xvfb',f':{host}','-screen','0','1600x1200x24','-nolisten','tcp','-ac'],stdout=output,stderr=output,env=env)
    try:
        for _ in range(100):
            if Path(f'/tmp/.X11-unix/X{host}').exists():break
            if xvfb.poll() is not None:raise RuntimeError((root/'xvfb.log').read_text())
            time.sleep(.05)
        for scale,dpi in [('0.5',1),('1',1),('1.25',1),('1.5',2),('2',1),('4',1),('auto',2)]:
            for rootless in (False,True):
                current={**env,'WINIT_X11_SCALE_FACTOR':str(dpi)}
                command=[str(binary),'serve','--scale',scale,'--core',str(core)]
                if rootless:command+=['--rootless']
                command+=['--',f':{display}','-nolisten','tcp']
                if not rootless:command+=['-screen','0','320x240x24','-crtcs','1@320x240']
                log=root/f'{scale}-{rootless}.log'
                with log.open('w') as output:
                    server=subprocess.Popen(command,stdout=output,stderr=output,env=current)
                try:
                    expected=str(dpi) if scale=='auto' else scale
                    subprocess.run([str(client),f':{display}',f':{host}',expected,str(int(rootless))],env=current,check=True,timeout=35)
                except BaseException:
                    print(log.read_text(errors='replace'),flush=True);raise
                finally:stop(server)
    finally:stop(xvfb)
