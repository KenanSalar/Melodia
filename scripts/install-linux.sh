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
#   3. Drops the SVG icon at
#      ~/.local/share/icons/hicolor/scalable/apps/melodia.svg
#   4. Best-effort desktop + icon cache refresh
#
# Both names match the RPM/DEB destinations — the desktop one so software
# centres merge it with the AppStream component, and both so switching
# install paths leaves no duplicate launcher or icon-cache entry.
#
# No sudo required, and the in-app updater needs no polkit prompt either:
# the install path is user-writable.
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

# Earlier per-user installs used these names; a leftover shows as a duplicate
# launcher for the same binary. Mirrored in uninstall-linux.sh.
rm -f "$APPS_DIR/Melodia.desktop" "$APPS_DIR/melodia.desktop"

# The AppStream MetaInfo isn't installed here — Melodia self-deploys it to
# ~/.local/share/metainfo/ on first launch (see desktop_integration.rs).

# LICENSE is informational. `licenses/` is not: the bundled fonts and the
# vendored winit fork compile into the binary, and Apache-2.0 §4(a) requires
# the text to travel with what it covers (see licenses/ATTRIBUTION.txt).
#
# Guard on the expansion, not the directory: nullglob is off, so an absent
# licenses/ leaves the pattern literal and `install` fails the whole script
# under `set -e` — halfway through, with the Exec rewrite still to come. `-D`
# creates the target directory, which plain `-t` will not.
LICENSE_FILES=("$SCRIPT_DIR"/licenses/*)
[[ -f "$SCRIPT_DIR/LICENSE" ]] && install -m 0644 "$SCRIPT_DIR/LICENSE" "$INSTALL_DIR/LICENSE"
[[ -e "${LICENSE_FILES[0]}" ]] && install -D -m 0644 -t "$INSTALL_DIR/licenses" "${LICENSE_FILES[@]}"

# Point Exec at the real install path — the tarballed .desktop assumes the
# binary is on $PATH. GNU sed (universal on the target distros) takes `-i`
# with no backup suffix.
#
# Only the command token, never the rest of the line: a `.*` here eats the
# ` %F` field code that makes file-opening work — silently, and only in the
# tarball, since the DEB ships this same source file verbatim.
#
# Exec is parsed with shell-like quoting, so a home directory with a space
# needs quotes or the launcher splits it into two arguments. Only when
# needed, an unquoted command being what every other source ships.
case "$INSTALL_DIR" in
  *\ *) EXEC_COMMAND="\"$INSTALL_DIR/Melodia\"" ;;
  *)    EXEC_COMMAND="$INSTALL_DIR/Melodia" ;;
esac
sed -i "s|^Exec=[^ ]*|Exec=$EXEC_COMMAND|" "$APPS_DIR/$DESKTOP_FILE"

# Best-effort: the session picks the changes up on next start regardless.
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
