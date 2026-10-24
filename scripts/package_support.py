"""Metadata, content verification and archives shared by runtime/test-kit builders."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import zipfile


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def stamp(root, kind, **extra):
    root = Path(root)
    target = next(line.removeprefix("host: ") for line in subprocess.check_output(
        ["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    sha = os.environ.get("GITHUB_SHA") or subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    abi = int(re.search(r"#define WAYX_ABI_VERSION (\d+)", Path("include/wayx.h").read_text())[1])
    manifest = dict(schema=1, kind=kind, source_sha=sha, platform=sys.platform,
                    arch=target.split("-")[0], target=target, abi=abi, **extra)
    manifest["files"] = {path.relative_to(root).as_posix(): digest(path)
                         for path in sorted(root.rglob("*")) if path.is_file() and path != root/"manifest.json"}
    (root/"manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def verify(root, expected_sha, kind):
    root = Path(root).resolve()
    manifest = json.loads((root/"manifest.json").read_text())
    arch = {"AMD64":"x86_64", "arm64":"aarch64"}.get(platform.machine(), platform.machine())
    if (manifest["schema"], manifest["kind"], manifest["source_sha"], manifest["platform"], manifest["arch"]) != (1, kind, expected_sha, sys.platform, arch):
        raise RuntimeError("Package identity does not match selected commit/platform/architecture")
    verify_files(root, manifest)
    return manifest


def verify_files(root, manifest):
    """Check contents independently of the host platform (also used for releases)."""
    root = Path(root).resolve()
    expected = manifest["files"]
    actual = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file() and p != root/"manifest.json"}
    if actual != set(expected):
        raise RuntimeError(f"Package file list differs: {actual.symmetric_difference(expected)}")
    for name, checksum in expected.items():
        path = (root/name).resolve()
        if not path.is_relative_to(root) or (root/name).is_symlink() or digest(path) != checksum:
            raise RuntimeError(f"Invalid packaged file: {name}")


def artifact_name(root, prefix, *, version=None):
    root = Path(root)
    manifest = json.loads((root/"manifest.json").read_text())
    label = {"win32":"windows", "darwin":"macos"}.get(manifest["platform"], manifest["platform"])
    arch = {"x86_64":"x64", "aarch64":"arm64"}.get(manifest["arch"])
    if arch is None: raise RuntimeError("Unsupported package architecture: "+manifest["arch"])
    if version is not None and not re.fullmatch(r"v[0-9][0-9A-Za-z.+-]*", version):
        raise ValueError("Expected a v-prefixed release version")
    identifier = version if version is not None else manifest["source_sha"]
    stem = f"{prefix}-{label}-{arch}-{identifier}"
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"artifact_name={stem}\n")
    return stem


def archive(root, directory, prefix, *, version=None):
    """Create a single ZIP with runtime contents at its root, for release assets."""
    root, directory = Path(root), Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((root/"manifest.json").read_text())
    path = directory/(artifact_name(root, prefix, version=version)+".zip")
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as output:
        for file in sorted(root.rglob("*")):
            if file.is_file():
                info = zipfile.ZipInfo.from_file(file, file.relative_to(root).as_posix())
                # Artifact downloads reset modes; restore executable entry points.
                if manifest["platform"] != "win32" and (file.parent == root/"bin" or file.name == manifest.get("launcher")):
                    info.create_system = 3
                    info.external_attr = 0o100755 << 16
                output.writestr(info, file.read_bytes(), compress_type=zipfile.ZIP_DEFLATED)
    print(path)
    return path


def elf_dependencies(source):
    """Resolve ELF dependencies on the build host; leave glibc/loader to the OS."""
    dependencies = subprocess.check_output(["ldd", str(source)], text=True)
    if "not found" in dependencies: raise RuntimeError(f"Unresolved dependencies of {source}: {dependencies}")
    result = []
    for line in dependencies.splitlines():
        match = re.search(r"(?:=> )?(/\S+) \(0x", line)
        if match:
            path = Path(match[1])
            system = bool(re.match(r"(?:ld-linux.*|lib(?:c|m|pthread|dl|rt|util|resolv|nss_[^.]*)\.so(?:\..*)?)$", path.name))
            result.append((path, system))
    return result
