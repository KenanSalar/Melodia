#!/usr/bin/env bash
# Build a relocatable AppImage from the existing `target/release/Melodia`
# binary using upstream `linuxdeploy` + `linuxdeploy-plugin-appimage`.
#
# Usage:
#   ./scripts/build-appimage.sh                       # x86_64, Melodia-<ver>-x86_64.AppImage
#   ./scripts/build-appimage.sh /tmp/Custom.AppImage  # x86_64, specific output path
#   ARCH=aarch64 ./scripts/build-appimage.sh          # aarch64, Melodia-<ver>-aarch64.AppImage
#
# Arch defaults to the runner's `uname -m`, so a developer on ARM64 Linux
# builds natively without ceremony; CI sets it per matrix slot.
#
# linuxdeploy's default exclude list is honoured: libGL / libEGL / libvulkan
# must come from the host driver, not the bundle, or accelerated rendering
# breaks wherever the GPU driver differs from the build runner's. Only the
# AppImageKit version and the output filename are overridden.

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

BINARY="${BINARY:-$REPO_ROOT/target/release/Melodia}"
[[ -f "$BINARY" ]] || { echo "ERROR: $BINARY not found. Run 'cargo build --release' first."; exit 1; }

# Both packages inherit the version from `[workspace.package]`, so `[package]`
# reads `version.workspace = true` and carries no literal. Anchor on the table
# rather than taking the file's first `version = ` line.
VERSION="$(awk -F'"' '
  /^\[/                  { in_ws = ($0 == "[workspace.package]") }
  in_ws && /^version = / { print $2; exit }
' Cargo.toml)"
[[ -n "$VERSION" ]] || { echo "ERROR: no version in Cargo.toml's [workspace.package]"; exit 1; }
ARCH="${ARCH:-$(uname -m)}"
case "$ARCH" in
  x86_64|aarch64) ;;
  *) echo "ERROR: unsupported ARCH=$ARCH (expected x86_64 or aarch64)"; exit 1 ;;
esac
OUTPUT="${1:-Melodia-${VERSION}-${ARCH}.AppImage}"

# Fetch linuxdeploy + the AppImage plugin if not cached.
#
# Pinned tags, not "continuous": a re-run two weeks later must produce a
# byte-identical AppImage modulo our own binary. The continuous channel rolls
# forward on every upstream commit, silently swapping the bundled runtime and
# plugin. Bump deliberately, and move the SHA256s for BOTH archs with it.
LINUXDEPLOY_TAG="1-alpha-20251107-1"
PLUGIN_TAG="1-alpha-20250213-1"
case "$ARCH" in
  x86_64)
    LINUXDEPLOY_SHA256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
    PLUGIN_SHA256="992d502a248e14ab185448ddf6f6e7d25558cb84d4623c354c3af350c25fccb3"
    ;;
  aarch64)
    LINUXDEPLOY_SHA256="620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff"
    PLUGIN_SHA256="83c292149274965a865dcd44c135cfca8ba28c6b7de3eb628d4b8b5f248af17c"
    ;;
esac

TOOLS_DIR="${LINUXDEPLOY_DIR:-$REPO_ROOT/target/appimage-tools}"
mkdir -p "$TOOLS_DIR"
LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-${ARCH}.AppImage"
PLUGIN="$TOOLS_DIR/linuxdeploy-plugin-appimage-${ARCH}.AppImage"

# Verify the cached copy still matches the pinned SHA before we trust
# it. Catches the case where a previous run cached a different tag's
# binary under the same filename (LINUXDEPLOY_DIR re-use across tag
# bumps). On mismatch we re-download.
verify_or_clear() {
  local file="$1" expected="$2"
  [[ -f "$file" ]] || return 1
  local actual
  actual="$(sha256sum "$file" | awk '{print $1}')"
  if [[ "$actual" == "$expected" ]]; then
    return 0
  fi
  echo "==> cached $(basename "$file") sha256 mismatch (have $actual, want $expected); re-downloading"
  rm -f "$file"
  return 1
}

if ! verify_or_clear "$LINUXDEPLOY" "$LINUXDEPLOY_SHA256"; then
  echo "==> downloading linuxdeploy $LINUXDEPLOY_TAG ($ARCH)"
  curl -fsSL -o "$LINUXDEPLOY" \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/${LINUXDEPLOY_TAG}/linuxdeploy-${ARCH}.AppImage"
  echo "${LINUXDEPLOY_SHA256}  $LINUXDEPLOY" | sha256sum -c -
  chmod +x "$LINUXDEPLOY"
fi
if ! verify_or_clear "$PLUGIN" "$PLUGIN_SHA256"; then
  echo "==> downloading linuxdeploy-plugin-appimage $PLUGIN_TAG ($ARCH)"
  curl -fsSL -o "$PLUGIN" \
    "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/${PLUGIN_TAG}/linuxdeploy-plugin-appimage-${ARCH}.AppImage"
  echo "${PLUGIN_SHA256}  $PLUGIN" | sha256sum -c -
  chmod +x "$PLUGIN"
fi

# Stage AppDir.
APPDIR="$(mktemp -d)/Melodia.AppDir"
trap 'rm -rf "$(dirname "$APPDIR")"' EXIT
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/256x256/apps"

cp "$BINARY" "$APPDIR/usr/bin/Melodia"
chmod +x "$APPDIR/usr/bin/Melodia"

# Icon — linuxdeploy uses the basename to derive `Icon=` in the desktop
# file, so the SVG filename must match the .desktop's Icon= field. Use
# the WITH-background brand SVG: it ships as the taskbar / launcher /
# alt-tab thumbnail icon where there's no app chrome behind the glyph.
# The without-background variant is reserved for the in-app custom
# titlebar where the window mantle already provides the disc.
cp "$REPO_ROOT/assets/icons/logo-with-background.svg" "$APPDIR/melodia.svg"

# Body kept in lockstep with `scripts/build-rpm.sh` + `Cargo.toml`'s
# DEB asset `scripts/Melodia.desktop` + `assets/desktop/Melodia.desktop.tmpl`.
# Drift here means AppImage users can't open audio files via "Open with…"
# and KDE shows two taskbar entries (no StartupWMClass to merge them).
# The MIME drift-guard test in
# `crates/melodia-platform/src/services/platform/tests/desktop_integration_tests.rs`
# covers the four sources — keep this body byte-identical bar the `Exec=`
# command: AppImage binaries sit at the AppDir root so `Melodia` is the relative
# name, RPM/DEB resolve `melodia` via PATH. The ` %F` is not optional; without it
# the MimeType list above is unusable.
# Desktop file named after the reverse-DNS app id so its desktop-id
# matches the AppStream component id `com.github.kenansalar.melodia`.
cat > "$APPDIR/com.github.kenansalar.melodia.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Melodia
GenericName=Music Player
Comment=Cross-platform desktop music player
Exec=Melodia %F
Icon=melodia
Categories=AudioVideo;Audio;Player;Music;
Keywords=music;audio;player;library;sound;songs;tracks;mp3;flac;ogg;
Terminal=false
StartupWMClass=Melodia
MimeType=audio/mpeg;audio/flac;audio/x-flac;audio/mp4;audio/x-m4a;audio/x-m4b;audio/ogg;audio/x-vorbis+ogg;audio/x-flac+ogg;audio/wav;audio/x-wav;audio/aac;audio/x-aac;audio/aiff;audio/x-aiff;audio/x-aifc;audio/x-matroska;
EOF

# AppStream MetaInfo — software centres and AppImage catalogues read
# this from usr/share/metainfo/ inside the bundle. appimagetool
# validates it automatically when appstreamcli is on $PATH. Shipped
# verbatim, identical to the RPM/DEB/tarball copies.
mkdir -p "$APPDIR/usr/share/metainfo"
cp "$REPO_ROOT/packaging/com.github.kenansalar.melodia.metainfo.xml" \
   "$APPDIR/usr/share/metainfo/com.github.kenansalar.melodia.metainfo.xml"

# Licences. The AppImage is self-contained — nothing resolves the MetaInfo's
# `<project_license>` back to a text — so it needs the AGPL itself, and the
# bundled fonts and the vendored winit fork are all compiled into the binary,
# which puts their licence text here under Apache-2.0 §4(a) rather than as a
# courtesy. Same destination the DEB uses. See licenses/ATTRIBUTION.txt.
mkdir -p "$APPDIR/usr/share/doc/melodia"
cp "$REPO_ROOT/LICENSE" "$APPDIR/usr/share/doc/melodia/LICENSE"
cp -r "$REPO_ROOT/licenses" "$APPDIR/usr/share/doc/melodia/licenses"

# Run linuxdeploy. `--output appimage` invokes the plugin to produce
# the final AppImage; without it we'd only get a populated AppDir.
echo "==> linuxdeploy → $OUTPUT"
PATH="$TOOLS_DIR:$PATH" \
LINUXDEPLOY_OUTPUT_VERSION="$VERSION" \
OUTPUT="$OUTPUT" \
"$LINUXDEPLOY" \
  --appdir "$APPDIR" \
  --desktop-file "$APPDIR/com.github.kenansalar.melodia.desktop" \
  --icon-file "$APPDIR/melodia.svg" \
  --executable "$APPDIR/usr/bin/Melodia" \
  --output appimage

echo "==> wrote $OUTPUT"
