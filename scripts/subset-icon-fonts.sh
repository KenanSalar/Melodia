#!/usr/bin/env bash
# Subset the two Material Symbols Rounded TTFs down to ONLY the icon ligatures
# Melodia renders, in BOTH the outlined (FILL=0) and filled (FILL=1) faces.
#
# Why this is non-trivial
# -----------------------
# Material Symbols reaches each icon through a GSUB *ligature*: the literal text
# "play_arrow" (a run of plain a–z/underscore glyphs) is substituted for the icon
# outline. An earlier version of this script subset with `--text="<all names>"`.
# That feeds the whole lowercase alphabet + underscore into pyftsubset's layout
# *closure*, and because every one of the ~3,600 icon names is spelled from those
# same letters, the closure re-added EVERY icon. Net result: the only thing that
# ever shrank was the variable-axis `gvar` table (dropped during instancing) —
# zero icon pruning. Both faces still shipped Google's entire icon set.
#
# This version subsets by an explicit glyph set with `--no-layout-closure`:
#   1. resolve each name in `scripts/icons.txt` to its ligature *output* glyph,
#   2. keep set = {letter/underscore component glyphs} ∪ {resolved icon glyphs},
#   3. retain the liga/calt/dlig lookups so "play_arrow" still shapes at runtime,
#      but DON'T let closure pull the rest of the catalogue back in.
#
# `scripts/icons.txt` is now load-bearing: an icon used in code but missing from
# it renders as a blank box (tofu). Keep it in sync — see `scripts/check-icons.py`,
# which re-derives the used set from the source tree and fails on drift.
#
# Source faces (pristine, multi-MiB, gitignored under scripts/fonts-src/)
# ----------------------------------------------------------------------
# The script reads its inputs from `scripts/fonts-src/` and writes the trimmed
# outputs to `ui/assets/fonts/` — it never overwrites its own input, so a face can
# always be re-subset (e.g. after adding an icon). Populate scripts/fonts-src/ with
# the full faces named exactly as the outputs:
#   scripts/fonts-src/MaterialSymbolsRounded.ttf
#   scripts/fonts-src/MaterialSymbolsRoundedFilled.ttf
# A single upstream *variable* MaterialSymbolsRounded[FILL,GRAD,opsz,wght].ttf
# (google/material-design-icons) can supply both: this script instances FILL=0 for
# the outlined face and FILL=1 for the filled one. Already-static sources (the
# previously-bundled faces are fine) skip the instancer.
#
# Prereqs:  pip install --user fonttools   (provides pyftsubset)
# Outputs (in place, committed):
#   ui/assets/fonts/MaterialSymbolsRounded.ttf
#   ui/assets/fonts/MaterialSymbolsRoundedFilled.ttf

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

ICONS="$REPO_ROOT/scripts/icons.txt"
SRC_DIR="$REPO_ROOT/scripts/fonts-src"
FONT_DIR="$REPO_ROOT/ui/assets/fonts"

command -v pyftsubset >/dev/null || {
  echo "ERROR: pyftsubset not on PATH. Install with: pip install --user fonttools"
  exit 1
}
[[ -f "$ICONS" ]] || { echo "ERROR: $ICONS not found"; exit 1; }

# subset_font <basename> <fill-axis-value>
subset_font() {
  local name="$1" fill="$2"
  local src="$SRC_DIR/$name" out="$FONT_DIR/$name"
  local tmp_inst="$src.inst.tmp" gids="$src.gids.tmp"
  local staged="$src"

  echo "==> $name (FILL=$fill)"
  [[ -f "$src" ]] || {
    echo "ERROR: source $src missing — populate scripts/fonts-src/ (see header)."
    exit 1
  }

  # Collapse a variable source to a static instance. Material Symbols default axes
  # are wght=400 / GRAD=0 / opsz=24; FILL selects the face (0 outlined, 1 filled).
  # Already-static sources (no fvar) skip this — `gvar` is what makes the variable
  # font ~14 MiB, and it's gone once instanced.
  if python3 -c "import sys; from fontTools.ttLib import TTFont; sys.exit(0 if 'fvar' in TTFont('$src') else 1)"; then
    python3 -m fontTools.varLib.instancer "$src" \
      --output="$tmp_inst" --no-recalc-timestamp \
      FILL="$fill" wght=400 GRAD=0 opsz=24
    staged="$tmp_inst"
  fi

  # Resolve ligature names -> output glyphs and emit the keep-set as glyph IDs.
  # Fails loudly if any name in icons.txt has no ligature in this face.
  FONT="$staged" ICONS="$ICONS" OUT_GIDS="$gids" python3 - <<'PY'
import os, sys
from fontTools.ttLib import TTFont

font = TTFont(os.environ["FONT"])
cmap = {}
for t in font["cmap"].tables:
    cmap.update(t.cmap)
char2g = {chr(cp): gn for cp, gn in cmap.items()}

# Ligature index: first-component glyph -> [(remaining components, output glyph)].
idx = {}
for lk in font["GSUB"].table.LookupList.Lookup:
    for st in lk.SubTable:
        if lk.LookupType == 7:          # unwrap extension lookups
            st = st.ExtSubTable
        for first, ligs in (getattr(st, "ligatures", None) or {}).items():
            for lig in ligs:
                idx.setdefault(first, []).append((tuple(lig.Component), lig.LigGlyph))

def resolve(name):
    if any(c not in char2g for c in name):
        return None
    gs = [char2g[c] for c in name]
    for comps, out in idx.get(gs[0], []):
        if comps == tuple(gs[1:]):
            return out
    return None

keep = set(cmap.values())               # component glyphs (a–z, underscore, …)
missing = []
for line in open(os.environ["ICONS"]):
    n = line.strip()
    if not n:
        continue
    g = resolve(n)
    if g is None:
        missing.append(n)
    else:
        keep.add(g)

if missing:
    sys.exit("ERROR: %s has no ligature for: %s" % (os.environ["FONT"], ", ".join(missing)))

gids = sorted({font.getGlyphID(g) for g in keep})
with open(os.environ["OUT_GIDS"], "w") as f:
    f.write("\n".join(map(str, gids)) + "\n")
print("    keep %d glyphs (source had %d)" % (len(gids), font["maxp"].numGlyphs))
PY

  # --no-layout-closure: keep exactly the glyphs above; retain liga/calt/dlig so
  # the kept ligatures still fire, without re-expanding to the full catalogue.
  pyftsubset "$staged" \
    --output-file="$out" \
    --gids-file="$gids" \
    --no-layout-closure \
    --layout-features+=liga,calt,dlig \
    --name-IDs='*' \
    --notdef-outline \
    --no-hinting \
    --drop-tables+=DSIG

  local before after
  before="$(stat -c %s "$src")"
  after="$(stat -c %s "$out")"
  printf "    %-40s %9d -> %9d bytes (%s%% of source)\n" \
    "$name" "$before" "$after" \
    "$(LC_ALL=C awk -v a="$after" -v b="$before" 'BEGIN { printf "%.1f", (a/b)*100 }')"

  rm -f "$tmp_inst" "$gids"
}

subset_font "MaterialSymbolsRounded.ttf"       0
subset_font "MaterialSymbolsRoundedFilled.ttf" 1

echo
echo "Done. Re-run \`cargo build --release\` to embed the new subsets."
