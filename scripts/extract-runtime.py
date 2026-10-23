#!/usr/bin/env python3
"""Verify the single runtime archive before testing its relocated contents."""
import argparse
import tarfile
import zipfile
from pathlib import Path
from package_support import digest

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archives',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
checksums=list(args.archives.glob('*.sha256'))
if len(checksums)!=1: raise RuntimeError('Expected exactly one runtime checksum')
checksum=checksums[0]
expected,name=checksum.read_text().strip().split(None,1)
archive=checksum.parent/name.strip()
if archive.parent.resolve()!=checksum.parent.resolve() or digest(archive)!=expected:
    raise RuntimeError('Invalid runtime archive name or checksum')
root=args.output.resolve()
root.mkdir(parents=True,exist_ok=False)
if archive.suffix=='.zip':
    with zipfile.ZipFile(archive) as bundle:
        if any(not (root/name).resolve().is_relative_to(root) for name in bundle.namelist()):
            raise RuntimeError('Invalid runtime archive path')
        bundle.extractall(root)
else:
    with tarfile.open(archive) as bundle: bundle.extractall(root,filter='data')
