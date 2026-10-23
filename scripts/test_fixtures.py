"""Strict resolver for precompiled package-test fixtures; local source tests may build."""
import json
import os
from pathlib import Path


def prebuilt(name):
    selected = os.environ.get("WAYX_TEST_KIT")
    if selected is None: return None
    root = Path(selected).resolve()
    manifest = json.loads((root/"manifest.json").read_text())
    name_in_kit = manifest["executables"].get(name)
    if not name_in_kit: raise RuntimeError(f"Required precompiled fixture missing: {name}")
    path = (root/name_in_kit).resolve()
    if not path.is_relative_to(root) or not path.is_file():
        raise RuntimeError(f"Invalid precompiled fixture: {name}")
    return path
