#!/usr/bin/env python3
"""Verify and copy runtime contents to a new location without nesting or archiving."""
import argparse
import shutil
from pathlib import Path
from package_support import verify

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--runtime", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--expected-sha", required=True)
args = parser.parse_args()
verify(args.runtime, args.expected_sha, "runtime")
shutil.copytree(args.runtime, args.output)
verify(args.output, args.expected_sha, "runtime")
