#!/usr/bin/env bash
# Native UCRT MinGW core using version-pinned vcpkg C dependencies.
set -euo pipefail
cd "$(dirname "$0")/.."
case "$(uname -s)" in MINGW*|MSYS*) ;; *) echo 'Run in a native MSYS2 UCRT64 shell' >&2;exit 1;; esac
: "${WAYX_WINDOWS_DEPS:?Set WAYX_WINDOWS_DEPS to the vcpkg x64-windows installed directory}"
deps="$(cygpath -m "$WAYX_WINDOWS_DEPS")"
prefix="$(cygpath -m "$PWD/build/windows-native")"
export PKG_CONFIG_LIBDIR="$prefix/share/pkgconfig;$prefix/lib/pkgconfig;$deps/lib/pkgconfig;$deps/share/pkgconfig"
export PATH="$(cygpath -u "$deps/bin"):$PATH"
export CC=gcc
fetch() { python3 scripts/fetch-build-source.py "$1" "$2" "$3" build/native-sources; }
build_dependency() {
  local dependency_name="$1" dependency_source="$2"
  shift 2
  meson setup "build/windows-$dependency_name" "$dependency_source" --prefix="$prefix" --libdir=lib --buildtype=release "$@"
  meson compile -C "build/windows-$dependency_name" -j "${WAYX_BUILD_JOBS:-3}"
  meson install -C "build/windows-$dependency_name"
}
# XLibre requires fixesproto >=6.1, newer than vcpkg's base xproto package.
protocols="$(fetch https://xorg.freedesktop.org/archive/individual/proto/xorgproto-2026.1.tar.gz 7fa90e48cbaca6bc4c99d176e88c5c147ab0ed42358e14a0869ee9c8a610ae7e xorgproto-2026.1)"
# Xpoll.h.in already implements native WinSock; its Meson probe only checks
# POSIX fd_set fields. Verify and select the WinSock field on native Windows.
python3 - "$protocols/include/X11/meson.build" <<'PY_PROTOCOLS'
from pathlib import Path
import sys
path = Path(sys.argv[1])
text = path.read_text()
old = "if cc.has_member('fd_set', 'fds_bits', prefix: fd_set_headers)"
new = """if host_machine.system() == 'windows'
    if not cc.has_member('fd_set', 'fd_array', prefix: '#include <winsock2.h>')
        error('Unsupported WinSock fd_set layout')
    endif
    fds_bits = 'fd_array'
elif cc.has_member('fd_set', 'fds_bits', prefix: fd_set_headers)"""
if new not in text:
    if text.count(old) != 1:
        raise SystemExit('Unexpected pinned xorgproto Meson probe')
    path.write_text(text.replace(old, new, 1))
PY_PROTOCOLS
build_dependency protocols "$protocols" -Dlegacy=true
compiler="$(fetch https://www.x.org/releases/individual/app/xkbcomp-1.5.0.tar.xz 2ac31f26600776db6d9cd79b3fcd272263faebac7eb85fb2f33c7141b8486060 xkbcomp-1.5.0)"
build_dependency xkbcomp "$compiler"
data="$(fetch https://xorg.freedesktop.org/archive/individual/data/xkeyboard-config/xkeyboard-config-2.48.tar.xz b77041324f0109f77161ee43743fe04baa485866af8460d31e476ad3f7648fd5 xkeyboard-config-2.48)"
build_dependency xkbdata "$data" -Dnls=false
export WAYX_XKB_BIN_DIR="$prefix/bin"
export WAYX_XKB_DIR="$prefix/share/xkeyboard-config-2"
export PATH="$(cygpath -u "$prefix/bin"):$PATH"
bash scripts/build-xlibre.sh
# Both halves use UCRT and the fixed C ABI. No C++/Rust objects cross DLL boundaries.
