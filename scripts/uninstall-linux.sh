#!/usr/bin/env bash
# Uninstall the per-user Melodia install created by install-linux.sh.
#
# Per-user data (queue.json, settings.json, melodia.db, artwork/) is
# left in place at $XDG_DATA_HOME/Melodia/ — that directory is shared
# with `dnf`-installed builds and contains the user's library, so we
# don't touch it. To wipe data too, follow the printed hint.
#
# Environment overrides:
#   MELODIA_INSTALL_DIR   — install dir (default: ~/.local/share/Melodia)
#   XDG_DATA_HOME         — base for ~/.local/share

set -euo pipefail

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
INSTALL_DIR="${MELODIA_INSTALL_DIR:-$DATA_HOME/Melodia}"
APPS_DIR="$DATA_HOME/applications"
ICONS_DIR="$DATA_HOME/icons/hicolor/scalable/apps"

removed=0
remove() {
  if [[ -e "$1" ]]; then
    rm -f "$1"
    echo "  removed $1"
    removed=$((removed + 1))
  fi
}

echo "==> uninstalling Melodia from $INSTALL_DIR"
remove "$INSTALL_DIR/Melodia"
remove "$INSTALL_DIR/LICENSE"
# Try to rmdir the install dir; non-fatal if user data is mixed in.
[[ -d "$INSTALL_DIR" ]] && rmdir --ignore-fail-on-non-empty "$INSTALL_DIR" 2>/dev/null || true

remove "$APPS_DIR/com.github.kenansalar.melodia.desktop"
# Legacy: earlier per-user installs used `Melodia.desktop` (capital M)
# and then `melodia.desktop`. Mirrors the `Melodia.svg` cleanup below.
remove "$APPS_DIR/Melodia.desktop"
remove "$APPS_DIR/melodia.desktop"
# AppStream MetaInfo — Melodia self-deploys this to the per-user
# metainfo dir on first launch (see desktop_integration.rs).
remove "$DATA_HOME/metainfo/com.github.kenansalar.melodia.metainfo.xml"
remove "$ICONS_DIR/melodia.svg"
# Legacy: earlier dev builds installed the icon as `Melodia.svg`
# (uppercase). Best-effort cleanup so a re-install with the corrected
# lowercase name doesn't leave a stale capitalised entry behind.
remove "$ICONS_DIR/Melodia.svg"

# Symlink under ~/.local/bin/ — only created if the user opted in
# during install (mentioned in install script's tail). Best-effort.
[[ -L "$HOME/.local/bin/melodia" ]] && remove "$HOME/.local/bin/melodia" || true

# Cache refresh.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q "$APPS_DIR" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$DATA_HOME/icons/hicolor" || true
fi

if [[ "$removed" -eq 0 ]]; then
  echo "Nothing to remove — Melodia wasn't installed at $INSTALL_DIR."
  exit 0
fi

cat <<EOF

✓ Uninstalled $removed file(s).

User data (library, settings, queue) is preserved at:
    $DATA_HOME/Melodia/

To wipe everything including data, also run:
    rm -rf "$DATA_HOME/Melodia"
EOF
