#!/usr/bin/env bash
#
# Regenerate the Discord Rich Presence art assets into assets/discord/.
#
# These are the images uploaded to the Discord Developer Portal for the Melodia
# application — the app icon/cover plus the small-badge art the presence code
# references by key. The icon/cover derive from the checked-in logo SVGs; the
# play/pause badges are drawn here procedurally (there is no SVG source for them),
# which is the whole reason this script exists: without it those two aren't
# reproducible.
#
# Portal mapping — file -> where it goes -> Rich Presence asset key (code const):
#   app-icon.png  General Information -> App Icon,  also RP Art Asset  key "melodia"
#   cover.png     Rich Presence -> Cover Image (16:9)                  (no key; cosmetic)
#   playing.png   RP Art Asset                                         key "playing"
#   paused.png    RP Art Asset                                         key "paused"
# The keys must match payload.rs (ASSET_LOGO/ASSET_PLAYING/ASSET_PAUSED). Discord
# lowercases keys on upload, so keep them lowercase here too.
#
# Requires: inkscape + ImageMagick (magick). Dev-only, like the font scripts —
# not part of the app build.

set -euo pipefail

# Repo root, regardless of where the script is invoked from.
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly SRC="$ROOT/ui/assets/icons"
readonly OUT="$ROOT/assets/discord"

# Brand tokens (Catppuccin Mocha): base background + the logo's blue->teal
# gradient stops, matching ml-grad in the logo SVGs (0,0 blue -> 48,48 teal).
readonly BG="#1e1e2e"
readonly GRAD_A="#89b4fa"
readonly GRAD_B="#94e2d5"

readonly TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$OUT"

# app-icon.png — the branded tile (rounded, transparent corners) flattened onto
# the base colour so it's a full 1024x1024 square (Discord rounds it on display).
inkscape "$SRC/logo-with-background.svg" \
  --export-type=png --export-area-page \
  --export-filename="$TMP/tile.png" -w 1024 -h 1024 >/dev/null 2>&1
magick -size 1024x1024 "xc:$BG" "$TMP/tile.png" -gravity center -composite "$OUT/app-icon.png"

# cover.png — the backdrop-free logo centred on a 16:9 base-colour canvas.
inkscape "$SRC/logo-without-background.svg" \
  --export-type=png --export-area-page \
  --export-filename="$TMP/logo.png" -w 576 -h 576 >/dev/null 2>&1
magick -size 1024x576 "xc:$BG" "$TMP/logo.png" -gravity center -composite "$OUT/cover.png"

# The two badges clip the same diagonal blue->teal gradient to a drawn white
# mask, then composite over BG. Gradient endpoints track each shape's box so the
# full colour range lands on it.

# paused.png — two rounded bars.
magick -size 1024x1024 xc:black -fill white \
  -draw "roundrectangle 312,296 456,728 48,48" \
  -draw "roundrectangle 568,296 712,728 48,48" "$TMP/pmask.png"
magick -size 1024x1024 xc: \
  -sparse-color barycentric "312,296 $GRAD_A 712,728 $GRAD_B" "$TMP/pgrad.png"
magick "$TMP/pgrad.png" \( "$TMP/pmask.png" -alpha off \) \
  -compose CopyOpacity -composite "$TMP/pshape.png"
magick -size 1024x1024 "xc:$BG" "$TMP/pshape.png" -compose over -composite "$OUT/paused.png"

# playing.png — a right-pointing play triangle with rounded corners.
magick -size 1024x1024 xc:black -fill white -stroke white -strokewidth 64 \
  -draw "stroke-linejoin round polygon 400,320 400,704 730,512" "$TMP/tmask.png"
magick -size 1024x1024 xc: \
  -sparse-color barycentric "400,320 $GRAD_A 730,704 $GRAD_B" "$TMP/tgrad.png"
magick "$TMP/tgrad.png" \( "$TMP/tmask.png" -alpha off \) \
  -compose CopyOpacity -composite "$TMP/tshape.png"
magick -size 1024x1024 "xc:$BG" "$TMP/tshape.png" -compose over -composite "$OUT/playing.png"

echo "Wrote Discord assets to $OUT:"
for f in app-icon cover playing paused; do
  printf '  %-12s %s\n' "$f.png" "$(magick identify -format '%wx%h' "$OUT/$f.png")"
done
