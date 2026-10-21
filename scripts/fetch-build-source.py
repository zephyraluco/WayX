#!/usr/bin/env python3
"""Fetch one pinned build dependency and reject corrupt/unsafe source archives."""
import hashlib
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import urllib.request

url, digest, name, directory = sys.argv[1:]
if Path(name).name != name or name in (".", ".."):
    raise SystemExit("Expected a single source directory name")
root = Path(directory).resolve()
target = root / name
stamp = target / ".wayx-source-sha256"
if target.exists():
    if not stamp.is_file() or stamp.read_text().strip() != digest:
        raise SystemExit(f"Refusing to reuse unverified source directory: {target}")
else:
    root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="wayx-fetch-", dir=root) as temporary:
        work = Path(temporary)
        archive = work / "archive"
        with urllib.request.urlopen(url, timeout=90) as source, archive.open("wb") as out:
            shutil.copyfileobj(source, out)
        with archive.open("rb") as downloaded:
            actual = hashlib.file_digest(downloaded, "sha256").hexdigest()
        if actual != digest:
            raise SystemExit(f"Source checksum mismatch: {name}")
        unpack = work / "unpack"
        with tarfile.open(archive) as source:
            source.extractall(unpack, filter="data")
        entries = list(unpack.iterdir())
        if len(entries) != 1 or not entries[0].is_dir() or entries[0].name != name:
            raise SystemExit(f"Unexpected source archive layout: {name}")
        entries[0].rename(target)
        stamp.write_text(digest + "\n")
print(target)
