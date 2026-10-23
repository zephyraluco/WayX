#!/usr/bin/env python3
"""Regression tests for artifact identity, integrity, names and strict fixture use."""
import json
import os
from pathlib import Path
import platform
import sys
import tempfile
import unittest
from unittest.mock import patch
from package_support import archive, digest, verify
from test_fixtures import prebuilt


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)/"runtime"
        self.root.mkdir()
        self.file = self.root/"fixture"
        self.file.write_text("binary stand-in")
        self.sha = "a"*40
        arch = {"AMD64":"x86_64", "arm64":"aarch64"}.get(platform.machine(), platform.machine())
        self.manifest = dict(schema=1, kind="runtime", source_sha=self.sha, platform=sys.platform, arch=arch,
                             files={"fixture":digest(self.file)})
        self.save()

    def save(self):
        (self.root/"manifest.json").write_text(json.dumps(self.manifest))

    def test_identity_and_content_are_required(self):
        verify(self.root, self.sha, "runtime")
        for key, value in [("source_sha", "b"*40), ("platform", "other"), ("arch", "other"), ("kind", "other")]:
            original = self.manifest[key]
            self.manifest[key] = value; self.save()
            with self.assertRaises(RuntimeError): verify(self.root, self.sha, "runtime")
            self.manifest[key] = original
        self.save(); self.file.write_text("changed")
        with self.assertRaises(RuntimeError): verify(self.root, self.sha, "runtime")

    def test_extra_and_missing_files_are_rejected(self):
        extra = self.root/"unexpected"
        extra.write_text("extra")
        with self.assertRaises(RuntimeError): verify(self.root, self.sha, "runtime")
        extra.unlink(); self.file.unlink()
        with self.assertRaises(RuntimeError): verify(self.root, self.sha, "runtime")

    def test_archive_uses_requested_name_and_checksum(self):
        with patch.dict(os.environ, {"GITHUB_OUTPUT":str(Path(self.directory.name)/"outputs")}):
            result = archive(self.root, Path(self.directory.name)/"archives", "core-runtime")
        label = {"win32":"windows", "darwin":"macos"}.get(sys.platform, sys.platform)
        arch = {"x86_64":"x64", "aarch64":"arm64"}[self.manifest["arch"]]
        self.assertTrue(result.name.startswith(f"{label}-core-runtime-{arch}-{self.sha}."))
        self.assertEqual(result.with_name(result.name+".sha256").read_text().split()[0], digest(result))

    def test_package_mode_never_falls_back_to_a_build(self):
        self.manifest["executables"] = {"ok":"fixture"}; self.save()
        with patch.dict(os.environ, {"WAYX_TEST_KIT":str(self.root)}):
            self.assertEqual(prebuilt("ok"), self.file.resolve())
            with self.assertRaisesRegex(RuntimeError, "missing"): prebuilt("not-packaged")

    @unittest.skipIf(os.name == "nt", "Directory symlinks require extra Windows privileges")
    def test_fixture_resolves_directory_aliases(self):
        self.manifest["executables"] = {"ok":"fixture"}; self.save()
        alias = Path(self.directory.name)/"alias"
        alias.symlink_to(self.root, target_is_directory=True)
        with patch.dict(os.environ, {"WAYX_TEST_KIT":str(alias)}):
            self.assertEqual(prebuilt("ok"), self.file.resolve())


if __name__ == "__main__": unittest.main()
