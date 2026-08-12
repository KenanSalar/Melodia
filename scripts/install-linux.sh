#!/usr/bin/env bash
# Per-user install script for the Melodia tarball release.
#
# Run this from inside the unpacked tarball directory:
#   tar -xzf melodia-v0.1.0-x86_64-linux.tar.gz
#   cd melodia
#   ./install-linux.sh
#
# What this does:
#   1. Copies the `Melodia` binary to ~/.local/share/Melodia/Melodia
#   2. Drops the .desktop file at
#      ~/.local/share/applications/com.github.kenansalar.melodia.desktop
#      (reverse-DNS — matches the RPM/DEB destination and the AppStream
#      component id, so software centres render the app correctly and a
#      user switching install paths doesn't get duplicate launchers.)
#   3. Drops the SVG icon at
#      ~/.local/share/icons/hicolor/scalable/apps/melodia.svg
#      (lowercase — matches the freedesktop hicolor icon-naming
#      convention and the RPM build, so a user switching between
#      install paths doesn't end up with two icon-cache entries.)
#   4. Refreshes the desktop + icon caches if the user has the tools
#      installed (best-effort; missing tools are not fatal)
#
# No sudo required. The in-app updater works directly against
# ~/.local/share/Melodia/Melodia without polkit prompts because that
# path is user-writable.
#
# Uninstall:
#   ./uninstall-linux.sh
#
# Environment overrides:
#   MELODIA_INSTALL_DIR   — install dir (default: ~/.local/share/Melodia)
#   XDG_DATA_HOME         — base for ~/.local/share

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
INSTALL_DIR="${MELODIA_INSTALL_DIR:-$DATA_HOME/Melodia}"
APPS_DIR="$DATA_HOME/applications"
ICONS_DIR="$DATA_HOME/icons/hicolor/scalable/apps"

# Verify the tarball contents we need are present next to this script.
need() {
  [[ -f "$SCRIPT_DIR/$1" ]] || { echo "ERROR: $1 missing — extracted tarball is incomplete." >&2; exit 1; }
}
need Melodia
need com.github.kenansalar.melodia.desktop
need melodia.svg

echo "==> installing Melodia to $INSTALL_DIR"
mkdir -p "$INSTALL_DIR" "$APPS_DIR" "$ICONS_DIR"

DESKTOP_FILE="com.github.kenansalar.melodia.desktop"
install -m 0755 "$SCRIPT_DIR/Melodia"           "$INSTALL_DIR/Melodia"
install -m 0644 "$SCRIPT_DIR/$DESKTOP_FILE"     "$APPS_DIR/$DESKTOP_FILE"
install -m 0644 "$SCRIPT_DIR/melodia.svg"       "$ICONS_DIR/melodia.svg"

# Legacy cleanup: earlier per-user installs used `Melodia.desktop`
# (capital M) and then `melodia.desktop`. If a previous install left
# one behind, remove it so the user doesn't end up with stale duplicate
# launcher entries pointing at the same binary. Mirrors the cleanup in
# uninstall-linux.sh.
rm -f "$APPS_DIR/Melodia.desktop" "$APPS_DIR/melodia.desktop"

# The AppStream MetaInfo (so the app shows correctly in KDE Discover /
# GNOME Software) is not installed here — Melodia self-deploys it to
# ~/.local/share/metainfo/ on first launch (see desktop_integration.rs).

# LICENSE is informational — drop it next to the binary so users can
# find it from the install dir. The bundled fonts and the vendored winit
# fork are compiled into the binary, so `licenses/` is not informational:
# Apache-2.0 §4(a) requires it to travel with what it covers. See
# licenses/ATTRIBUTION.txt.
#
# Guard on the expansion rather than on the directory: nullglob is off, so
# an empty (or absent) licenses/ leaves the pattern literal, and `install`
# would then fail the whole script under `set -e` — halfway through, with the
# .desktop Exec rewrite and the cache refresh below still to run. `-D` creates
# the target directory, which plain `-t` will not.
LICENSE_FILES=("$SCRIPT_DIR"/licenses/*)
[[ -f "$SCRIPT_DIR/LICENSE" ]] && install -m 0644 "$SCRIPT_DIR/LICENSE" "$INSTALL_DIR/LICENSE"
[[ -e "${LICENSE_FILES[0]}" ]] && install -D -m 0644 -t "$INSTALL_DIR/licenses" "${LICENSE_FILES[@]}"

# Rewrite the .desktop file's Exec line to point at the actual install
# path. The tarballed .desktop assumes the binary is on $PATH; we make
# it absolute so the launcher works regardless of $PATH layout. GNU
# sed (universal on the target distros — Fedora, Debian, Ubuntu,
# openSUSE) supports `-i` without a backup suffix.
sed -i "s|^Exec=.*|Exec=$INSTALL_DIR/Melodia|" "$APPS_DIR/$DESKTOP_FILE"

# Best-effort cache refresh. None of these are required — the desktop
# environment will pick up the changes on next session start either
# way.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q "$APPS_DIR" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$DATA_HOME/icons/hicolor" || true
fi

cat <<EOF

✓ Melodia installed.

  Binary    : $INSTALL_DIR/Melodia
  Launcher  : $APPS_DIR/$DESKTOP_FILE
  Icon      : $ICONS_DIR/melodia.svg

Run it from your application launcher, or directly:
    $INSTALL_DIR/Melodia

Optional: add Melodia to your PATH:
    mkdir -p \$HOME/.local/bin
    ln -sf "$INSTALL_DIR/Melodia" \$HOME/.local/bin/melodia

Uninstall: ./uninstall-linux.sh (or rm -rf $INSTALL_DIR and the .desktop/icon)
EOF
