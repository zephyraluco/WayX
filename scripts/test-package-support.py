#!/usr/bin/env python3
"""Regression tests for artifact identity, integrity, names and strict fixture use."""
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from unittest.mock import patch
from package_support import archive, artifact_name, digest, verify
from test_fixtures import prebuilt


class RunnerEncodingTests(unittest.TestCase):
    def check_runner(self, failure):
        # Exercise the actual runner with redirected CP1252 streams, as on Windows.
        runner = Path(__file__).with_name("test-package.py").resolve()
        harness = '''
import os, runpy, subprocess, sys
from unittest.mock import patch
runner, directory, failure = sys.argv[1:]
sys.path.insert(0, os.path.dirname(runner))
manifest = dict(abi=5, target="fixture", core="core", client="client", launcher="launcher")
tests = dict(manifest, runtime_manifest_sha256="fixture", units=[],
             executables={name:name for name in ("xlibre", "rootless", "window_manager", "desktop_layout")})
def run(command, **kwargs):
    assert kwargs["env"]["PYTHONIOENCODING"] == "utf-8"
    os.write(kwargs["stdout"].fileno(), "2\\u21923\\u21921 outputs; \\u4e2d\\u6587\\n".encode("utf-8"))
    if failure == "yes":
        raise subprocess.CalledProcessError(7, command)
sys.argv = [runner, "--runtime", directory, "--test-kit", directory,
            "--expected-sha", "a"*40, "--logs", directory]
with patch("package_support.verify", side_effect=[manifest, tests]), \
     patch("package_support.digest", return_value="fixture"), \
     patch("subprocess.run", side_effect=run), patch("sys.platform", "win32"):
    try:
        runpy.run_path(runner, run_name="__main__")
    except subprocess.CalledProcessError as error:
        sys.exit(error.returncode)
'''
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, "-c", harness, str(runner), directory, "yes" if failure else "no"],
                env={**os.environ, "PYTHONIOENCODING":"cp1252", "PYTHONUTF8":"0"},
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            self.assertEqual(result.returncode, 7 if failure else 0, result.stderr.decode("utf-8", errors="replace"))
            self.assertIn("2→3→1 outputs; 中文".encode(), result.stdout)
            self.assertEqual((Path(directory)/"package-support.log").read_text(encoding="utf-8"),
                             "2→3→1 outputs; 中文\n")
            self.assertNotIn(b"UnicodeEncodeError", result.stderr)

    def test_unicode_log_replay_with_legacy_console(self):
        self.check_runner(failure=False)

    def test_unicode_log_replay_preserves_failed_command(self):
        self.check_runner(failure=True)


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

    def test_archive_has_flat_contents_without_another_archive_or_checksum(self):
        binary = self.root/"bin"/"wayx"
        binary.parent.mkdir()
        binary.write_bytes(b"runtime binary")
        self.manifest["files"]["bin/wayx"] = digest(binary)
        self.save()
        with patch.dict(os.environ, {"GITHUB_OUTPUT":str(Path(self.directory.name)/"outputs")}):
            result = archive(self.root, Path(self.directory.name)/"archives", "core-runtime")
        label = {"win32":"windows", "darwin":"macos"}.get(sys.platform, sys.platform)
        arch = {"x86_64":"x64", "aarch64":"arm64"}[self.manifest["arch"]]
        self.assertEqual(result.name, f"{label}-core-runtime-{arch}-{self.sha}.zip")
        self.assertEqual(list(result.parent.iterdir()), [result])
        with zipfile.ZipFile(result) as bundle:
            self.assertEqual(set(bundle.namelist()), {"fixture", "manifest.json", "bin/wayx"})
            self.assertEqual(bundle.read("bin/wayx"), binary.read_bytes())
            if sys.platform != "win32":
                self.assertEqual(bundle.getinfo("bin/wayx").external_attr >> 16, 0o100755)

    def test_artifact_name_does_not_create_an_archive(self):
        with patch.dict(os.environ, {"GITHUB_OUTPUT":str(Path(self.directory.name)/"outputs")}):
            name = artifact_name(self.root, "core-runtime")
        self.assertEqual((Path(self.directory.name)/"outputs").read_text(), f"artifact_name={name}\n")
        self.assertEqual({p.name for p in self.root.iterdir()}, {"fixture", "manifest.json"})

    def test_relocation_is_flat_and_verifies_contents(self):
        script = Path(__file__).with_name("relocate-runtime.py")
        output = Path(self.directory.name)/"relocated runtime"
        command = [sys.executable, str(script), "--runtime", str(self.root), "--output", str(output),
                   "--expected-sha", self.sha]
        subprocess.run(command, check=True, capture_output=True)
        verify(output, self.sha, "runtime")
        self.assertEqual({p.name for p in output.iterdir()}, {"fixture", "manifest.json"})
        self.file.write_text("corrupted")
        shutil.rmtree(output)
        result = subprocess.run(command, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())

    def test_release_archives_verify_all_three_platforms(self):
        packages = Path(self.directory.name)/"packages"
        packages.mkdir()
        for platform_name, label in (("linux", "linux"), ("win32", "windows"), ("darwin", "macos")):
            root = packages/f"{label}-core-runtime-x64-{self.sha}"
            shutil.copytree(self.root, root)
            manifest = dict(self.manifest, platform=platform_name, arch="x86_64")
            (root/"manifest.json").write_text(json.dumps(manifest))
        output = Path(self.directory.name)/"release-assets"
        command = [sys.executable, str(Path(__file__).with_name("prepare-release.py")),
                   "--packages", str(packages), "--output", str(output), "--expected-sha", self.sha]
        subprocess.run(command, check=True, capture_output=True)
        self.assertEqual(len(list(output.glob("*.zip"))), 3)
        for path in output.iterdir():
            with zipfile.ZipFile(path) as bundle:
                self.assertEqual(set(bundle.namelist()), {"fixture", "manifest.json"})
        shutil.rmtree(output)
        next(packages.glob("*/fixture")).write_text("corrupted")
        result = subprocess.run(command, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())

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
