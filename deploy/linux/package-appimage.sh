#!/usr/bin/env bash
# Build a single-file AppImage of the EndlessVibe CLI/MCP service.
# Requires appimagetool supplied by the builder; never downloads or runs tools implicitly.
set -Eeuo pipefail

repo="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
output_dir="$repo/dist/linux"
tool="${APPIMAGETOOL:-}"
runtime_file="${APPIMAGETOOL_RUNTIME_FILE:-}"
offline=0
skip_build=0
no_smoke=0

usage() {
    cat <<'HELP'
Usage: ./deploy/linux/package-appimage.sh [options]

Builds dist/linux/EndlessVibe-VERSION-ARCH.AppImage (one distributable file).
AppImage packages the CLI daemon; arguments including --init pass through.

Options:
  --appimagetool FILE   Official appimagetool binary or .AppImage
  --runtime-file FILE   Pre-downloaded Type 2 runtime (for offline builds)
  --output-dir DIR      Output directory (default: dist/linux)
  --offline             Pass --offline to cargo
  --skip-build          Reuse target/release/endlessvibe (checks --version)
  --no-smoke            Skip executing the final AppImage (e.g. foreign arch)
  -h, --help            Show this help

Install appimagetool first or set APPIMAGETOOL to its path:
  https://github.com/AppImage/appimagetool/releases/continuous

An AppImage still needs a compatible host kernel/glibc; build on an older
supported Linux distribution for broad compatibility.
HELP
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
info() { printf '[appimage] %s\n' "$*"; }
while (($#)); do
    case "$1" in
        --appimagetool|--runtime-file|--output-dir)
            (($# >= 2)) || die "$1 needs a value"
            case "$1" in
                --appimagetool) tool="$2" ;;
                --runtime-file) runtime_file="$2" ;;
                --output-dir) output_dir="$2" ;;
            esac
            shift 2 ;;
        --offline) offline=1; shift ;;
        --skip-build) skip_build=1; shift ;;
        --no-smoke) no_smoke=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (use --help)" ;;
    esac
done

[[ "$(uname -s)" == Linux ]] || die "AppImages must be built on Linux"
arch="$(uname -m)"
case "$arch" in
    x86_64) target="x86_64-unknown-linux-gnu" ;;
    aarch64) target="aarch64-unknown-linux-gnu" ;;
    *) die "unsupported Linux architecture: $arch (supported: x86_64, aarch64)" ;;
esac
for cmd in awk cargo install readelf sha256sum mktemp chmod cp ln mkdir; do
    command -v "$cmd" >/dev/null 2>&1 || die "missing required command: $cmd"
done
version="$(awk -F '"' '/^\[package\]/{in_package=1;next} /^\[/{in_package=0} in_package && /^version[[:space:]]*=/{print $2;exit}' "$repo/Cargo.toml")"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][a-zA-Z0-9.-]+)?$ ]] || die "cannot parse Cargo package version"
[[ -f "$repo/EndlessVibe.png" ]] || die "missing embedded application icon EndlessVibe.png"
[[ -f "$repo/LICENSE" ]] || die "missing LICENSE"

if [[ -z "$tool" ]]; then
    tool="$(command -v appimagetool || true)"
fi
[[ -n "$tool" ]] || die "appimagetool not found. Install it or use --appimagetool FILE. See --help."
[[ -x "$tool" ]] || die "appimagetool is not executable: $tool"
if [[ -n "$runtime_file" ]]; then
    [[ -r "$runtime_file" ]] || die "runtime file is not readable: $runtime_file"
    runtime_file="$(cd -P -- "$(dirname "$runtime_file")" && pwd)/$(basename "$runtime_file")"
fi

binary="$repo/target/release/endlessvibe"
if ((skip_build == 0)); then
    info "building the release executable ($target) with a locked dependency graph"
    args=(build --locked --release --target "$target")
    ((offline == 0)) || args+=(--offline)
    (cd "$repo" && cargo "${args[@]}")
    binary="$repo/target/$target/release/endlessvibe"
fi
[[ -x "$binary" ]] || die "missing release binary: $binary"
actual_version="$("$binary" --version)"
[[ "$actual_version" == "EndlessVibe $version" ]] || die "binary/manifest version mismatch: $actual_version vs $version"
"$binary" --help >/dev/null || die "built EXE failed the --help smoke test"

# AppImage includes the service and embedded UI; do not package private data.
# Fail instead of silently shipping unresolved non-base shared libraries.
while IFS= read -r needed; do
    [[ -n "$needed" ]] || continue
    case "$needed" in
        libc.so.6|libm.so.6|libgcc_s.so.1|libpthread.so.0|libdl.so.2|librt.so.1|libutil.so.1|libresolv.so.2|libanl.so.1|ld-linux-*.so.*|ld-linux-aarch64.so.*) ;;
        *) die "unexpected ELF dependency: $needed. Bundle/audit it explicitly before publishing." ;;
    esac
done < <(readelf -d "$binary" | awk '/\(NEEDED\)/{gsub(/\[|\]/,"",$NF);print $NF}')

build_root="$repo/target/package-appimage"
mkdir -p -- "$build_root" "$output_dir"
output_dir="$(cd -P -- "$output_dir" && pwd)"
appdir="$(mktemp -d "$build_root/AppDir.XXXXXXXX")"
cleanup() { [[ -z "${appdir:-}" ]] || rm -rf -- "$appdir"; }
trap cleanup EXIT

mkdir -p -- "$appdir/usr/bin" "$appdir/usr/share/licenses/endlessvibe" "$appdir/usr/share/pixmaps"
install -m 755 -- "$binary" "$appdir/usr/bin/endlessvibe"
install -m 644 -- "$repo/LICENSE" "$appdir/usr/share/licenses/endlessvibe/LICENSE"
install -m 644 -- "$repo/EndlessVibe.png" "$appdir/endlessvibe.png"
ln -s endlessvibe.png "$appdir/.DirIcon"
ln -s ../../../endlessvibe.png "$appdir/usr/share/pixmaps/endlessvibe.png"

cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
set -eu
appdir="${APPDIR:-$(CDPATH='' cd -P -- "$(dirname -- "$0")" && pwd)}"
exec "$appdir/usr/bin/endlessvibe" "$@"
APPRUN
chmod 755 "$appdir/AppRun"
cat > "$appdir/endlessvibe.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=EndlessVibe
Comment=Self-hosted MCP development service
Exec=endlessvibe
Icon=endlessvibe
Terminal=true
Categories=Development;Utility;
X-AppImage-Version=$version
DESKTOP

# Run the bundled entry point before creating the AppImage.
[[ "$("$appdir/AppRun" --version)" == "EndlessVibe $version" ]] || die "AppRun smoke test failed"
output="$output_dir/EndlessVibe-$version-$arch.AppImage"
[[ ! -d "$output" ]] || die "output is an existing directory: $output"
info "creating $output"
# APPIMAGE_EXTRACT_AND_RUN lets a packaged appimagetool work on CI hosts
# without FUSE. APPIMAGETOOL_RUNTIME_FILE avoids implicit runtime downloads.
if [[ -n "$runtime_file" ]]; then
    ARCH="$arch" VERSION="$version" APPIMAGE_EXTRACT_AND_RUN=1 \
        APPIMAGETOOL_RUNTIME_FILE="$runtime_file" \
        "$tool" "$appdir" "$output"
else
    ARCH="$arch" VERSION="$version" APPIMAGE_EXTRACT_AND_RUN=1 \
        "$tool" "$appdir" "$output"
fi
[[ -s "$output" ]] || die "appimagetool did not produce the requested AppImage"
chmod 755 "$output"
if ((no_smoke == 0)); then
    info "checking the final AppImage with --version / --help"
    result="$(APPIMAGE_EXTRACT_AND_RUN=1 "$output" --version)" ||
        die "AppImage smoke test failed; check runtime/FUSE compatibility"
    [[ "$result" == "EndlessVibe $version" ]] || die "AppImage reported an unexpected version: $result"
    APPIMAGE_EXTRACT_AND_RUN=1 "$output" --help >/dev/null ||
        die "AppImage --help smoke test failed"
fi
info "created: $output"
info "sha256: $(sha256sum "$output")"
info "usage: chmod +x '$output' && '$output' --init"
