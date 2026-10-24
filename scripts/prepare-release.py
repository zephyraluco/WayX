#!/usr/bin/env python3
"""Verify downloaded runtime contents and create single-layer release ZIPs."""
import argparse
import json
from pathlib import Path
from package_support import archive, artifact_name, verify_files

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--packages", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--expected-sha", required=True)
parser.add_argument("--version", required=True, help="Release tag, e.g. v0.1.0")
args = parser.parse_args()
seen = set()
roots = sorted(args.packages.iterdir())
if len(roots) != 3:
    raise RuntimeError("Expected exactly three runtime artifacts")
for root in roots:
    manifest = json.loads((root/"manifest.json").read_text())
    platform = manifest["platform"]
    if (manifest["schema"] != 1 or manifest["kind"] != "runtime" or
            manifest["source_sha"] != args.expected_sha or
            platform not in {"linux", "win32", "darwin"} or platform in seen):
        raise RuntimeError("Runtime package identity mismatch")
    if root.name != artifact_name(root, "wayx"):
        raise RuntimeError("Runtime artifact name mismatch")
    verify_files(root, manifest)
    seen.add(platform)
for root in roots:
    archive(root, args.output, "wayx", version=args.version)
