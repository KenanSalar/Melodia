#!/usr/bin/env bash
# Subset the Material Symbols Rounded TTFs to only the icon ligatures
# Melodia actually uses, plus a handful of "planned but not yet
# implemented" icons (e.g. mini-player). Drops the on-disk size of the
# variable-axis font from ~14 MiB to ~1 MiB by freezing all axes at
# defaults and pruning unused glyphs.
#
# Run from the repo root after the icon list (`scripts/icons.txt`) is
# updated. The script overwrites the bundled TTFs in place. Slint's
# `build.rs` picks up the new smaller files automatically — no other
# wiring change is needed.
#
# Prereqs:
#   pip install --user fonttools  (provides pyftsubset)
#
# Inputs:
#   ui/assets/fonts/MaterialSymbolsRounded.ttf        (variable, ~14 MiB)
#   ui/assets/fonts/MaterialSymbolsRoundedFilled.ttf  (static FILL=1, ~1.7 MiB)
#   scripts/icons.txt                                  (newline-separated ligature names)
#
# Outputs (in place):
#   ui/assets/fonts/MaterialSymbolsRounded.ttf
#   ui/assets/fonts/MaterialSymbolsRoundedFilled.ttf

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

ICONS="$REPO_ROOT/scripts/icons.txt"
FONT_DIR="$REPO_ROOT/ui/assets/fonts"
[[ -f "$ICONS" ]] || { echo "ERROR: $ICONS not found"; exit 1; }
command -v pyftsubset >/dev/null || {
  echo "ERROR: pyftsubset not on PATH. Install with: pip install --user fonttools"
  exit 1
}

# Material Symbols ligature names use underscores and lowercase letters
# only. Joining the names with spaces gives the union of needed
# letters/underscores AND lets the GSUB ligature lookups match the
# full names.
TEXT="$(tr '\n' ' ' < "$ICONS")"

subset_font() {
  local src="$1"
  local is_variable="$2"   # "yes" or "no"
  local tmp="$src.tmp"
  local staged="$src"

  echo "==> processing $(basename "$src")"

  # If the font is variable (has fvar / gvar), collapse it to a static
  # instance at default axis values first. MaterialIcon.slint sets only
  # font-family + font-size — no axis overrides — so we lose nothing
  # by freezing FILL/wght/GRAD/opsz.
  #
  # Without explicit axis values, `varLib.instancer` just copies the
  # font — it only drops `gvar` for axes you explicitly pin. The
  # default axis values for Material Symbols Rounded are:
  #   FILL=0   (outlined; the Filled variant lives in a separate file)
  #   wght=400 (regular weight)
  #   GRAD=0   (default grade)
  #   opsz=24  (default optical size — Slint sizes via font-size,
  #            no need for opsz interpolation)
  # `gvar` alone is ~13 MiB on the Rounded font and disappears
  # entirely after instancing.
  if [[ "$is_variable" == "yes" ]]; then
    python3 -m fontTools.varLib.instancer \
      "$src" \
      --output="$tmp" \
      --no-recalc-timestamp \
      FILL=0 wght=400 GRAD=0 opsz=24
    staged="$tmp"
  fi

  local out="${src%.ttf}.subset.ttf"
  # `--layout-features+=liga,calt,dlig` keeps the GSUB lookups that
  # turn the typed text "play_arrow" into the icon glyph. pyftsubset
  # then prunes lookups for ligatures whose output glyphs aren't in
  # the subset, dramatically shrinking the GSUB table.
  # `--no-hinting` drops TrueType hinting (no visible effect at icon
  # sizes 22–48 px on FemtoVG's rendered output).
  # `--drop-tables+=DSIG` removes the empty digital-signature table.
  pyftsubset "$staged" \
    --output-file="$out" \
    --text="$TEXT" \
    --layout-features+=liga,calt,dlig \
    --name-IDs='*' \
    --notdef-outline \
    --no-hinting \
    --drop-tables+=DSIG

  local before after
  before="$(stat -c %s "$src")"
  after="$(stat -c %s "$out")"
  printf "    %-42s %10d → %10d bytes (%s%% of original)\n" \
    "$(basename "$src")" "$before" "$after" \
    "$(LC_ALL=C awk -v a="$after" -v b="$before" 'BEGIN { printf "%.1f", (a/b)*100 }')"

  mv "$out" "$src"
  rm -f "$tmp"
}

subset_font "$FONT_DIR/MaterialSymbolsRounded.ttf"       yes
subset_font "$FONT_DIR/MaterialSymbolsRoundedFilled.ttf" no

echo
echo "Done. Re-run \`cargo build --release\` to embed the new subsets."
