#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
source_dir="${WAYX_XLIBRE_SOURCE:-$project_root/build/xlibre-source}"
build_dir="${WAYX_XLIBRE_BUILD:-$project_root/build/xlibre}"
commit=a8192aa70998a803155bdc2e73b7564a1415f33a
if [[ ! -d "$source_dir/.git" ]]; then
  mkdir -p "$(dirname "$source_dir")"
  if [[ -d "$source_dir" && -n "$(find "$source_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo 'Refusing to initialize XLibre in a nonempty, non-Git directory' >&2
    exit 1
  fi
  git init "$source_dir"
  git -C "$source_dir" remote add origin https://github.com/X11Libre/xserver.git
  git -C "$source_dir" fetch --depth 1 origin "$commit"
  git -C "$source_dir" checkout --detach FETCH_HEAD
fi
[[ "$(git -C "$source_dir" rev-parse HEAD)" == "$commit" ]] || { echo 'Unexpected XLibre revision' >&2; exit 1; }
# Use a dedicated generated DDX directory; never replace upstream DDX sources.
mkdir -p "$source_dir/hw/wayx"
cp "$project_root/ddx/"*.c "$project_root/ddx/"*.h "$project_root/ddx/meson.build" "$project_root/include/wayx.h" "$source_dir/hw/wayx/"
python3 - "$source_dir/hw/meson.build" <<'PY'
from pathlib import Path
import sys
path=Path(sys.argv[1]); text=path.read_text(); hook="subdir('wayx')"
if hook not in text: path.write_text(text+'\n'+hook+'\n')
PY
# Native dependencies must be available through pkg-config; no authentication or
# package-signature checks are disabled. Let pkg-config locate native XKB data.
args=(--buildtype=debugoptimized -Dxorg=false -Dxnest=false -Dxwin=false -Dxquartz=false
      -Dxvfb=true -Dglamor=false -Dglx=false -Dglx_dri=false -Dgbm=false -Ddrm=false
      -Ddri1=false -Ddri2=false -Ddri3=false -Dudev=false -Dudev_kms=false
      -Dseatd_libseat=false -Dxselinux=false -Dxdmcp=false -Dinput_thread=false
      -Dxdm-auth-1=false -Dtests=false -Ddefault_font_path=built-ins)
# Meson discovers xkbcomp's bindir/xkbconfigdir on Linux, Homebrew and MSYS2.
# The generated keymaps must go to a writable directory, not the data prefix.
xkb_output="${WAYX_XKB_OUTPUT_DIR:-$(python3 -c 'import tempfile; print(tempfile.gettempdir())')}"
args+=("-Dxkb_output_dir=$xkb_output")
[[ -z "${WAYX_XKB_BIN_DIR:-}" ]] || args+=("-Dxkb_bin_dir=$WAYX_XKB_BIN_DIR")
[[ -z "${WAYX_XKB_DIR:-}" ]] || args+=("-Dxkb_dir=$WAYX_XKB_DIR")
case "$(uname -s)" in
  MINGW*|MSYS*)
    python3 "$project_root/scripts/patch-xlibre-windows.py" "$source_dir"
    args+=(-Dmitshm=false)
    ;;
esac
if [[ -f "$build_dir/build.ninja" ]]; then
  meson setup --reconfigure "$build_dir" "$source_dir" "${args[@]}"
else
  meson setup "$build_dir" "$source_dir" "${args[@]}"
fi
meson compile -C "$build_dir" wayx-xserver -j "${WAYX_BUILD_JOBS:-3}" --ninja-args=-k0
# Do not assume Linux's .so naming on Darwin or Windows.
meson introspect --targets "$build_dir" > "$build_dir/wayx-targets.json"
python3 - "$build_dir" <<'PY_ARTIFACT'
from pathlib import Path
import json, sys
build = Path(sys.argv[1])
targets = json.loads((build / "wayx-targets.json").read_text())
files = [Path(name) for target in targets if target["name"] == "wayx-xserver"
         for name in target["filename"] if Path(name).suffix in (".so", ".dylib", ".dll")]
if len(files) != 1 or not files[0].is_file():
    raise SystemExit("Expected one built WayX core shared library")
path = files[0].resolve()
(build / "wayx-core-path.txt").write_text(str(path) + "\n")
print(f"XLibre DDX plugin: {path}")
PY_ARTIFACT
