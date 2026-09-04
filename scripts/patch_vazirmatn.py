#!/usr/bin/env python3
"""Generate the patched Vazirmatn variant with rewritten line-metrics.

Upstream Vazirmatn ships with OS/2 typo + hhea ascent/descent sized to clear
Arabic combining marks (ratio ~1.56x font-size). At the app's UI sizes that
shifts every vertically-centred Latin text upward by several px because the
typo line box is much taller than typical Latin UI fonts (~1.20x).

This script rewrites only the layout-metric fields. Glyph outlines,
capHeight, x-height, and the usWin* clip bounds stay untouched, so glyph
rendering is unchanged.

The cost is that the ink now leaves the box: outlines still reach yMax 2163
and yMin -1160 against a box of 1650/-500, so Arabic marks sit a quarter of
an em above it and a third of an em below. That is fine while nothing cuts a
text run to its own bounds -- and it is exactly why nothing may. Two things
in Slint do it by default and neither is visible on a Latin locale: a
sub-unit `opacity`, which rasterizes into a texture sized to child geometry,
and a `Text` left on the default `overflow: clip`, which scissors itself.
Both rules, and the fixes, are in `.claude/rules/slint-pitfalls.md`.

  typoAsc=1650, typoDesc=-500  (~1.05x line ratio at UPM=2048)

The asymmetric box (1650 above the baseline, 500 below) lands the glyph
ink mass at the line-box centre on FemtoVG, so Slint's
`vertical-alignment: center` reads as optically centred -- this matters
most in tight chromes with a background fill (pill buttons, the settings
section buttons), where any bias is obvious. Tuning: lowering typoAsc
lifts text up, raising it drops text down -- a ~150-unit step is roughly
half a pixel at the app's UI font sizes (`dShift = fontSize / (2 * UPM)
* dTypoAsc`).

Output layout (all paths under crates/melodia-ui/ui/assets/fonts/):
  originals/Vazirmatn-*.ttf   pristine upstream copy (not committed -- re-download to update)
  vazirmatn/Vazirmatn-*.ttf   patched, imported directly by the tree's app-window.slint

The script is idempotent: re-running always reads from originals/ and writes
the same patched outputs.

Usage:
    python3 scripts/patch_vazirmatn.py
"""

import shutil
from pathlib import Path

from fontTools.ttLib import TTFont

METRICS = {"typo_asc": 1650, "typo_desc": -500}

REPO_ROOT = Path(__file__).resolve().parents[1]
FONT_DIR = REPO_ROOT / "crates" / "melodia-ui" / "ui" / "assets" / "fonts"
ORIG_DIR = FONT_DIR / "originals"
OUT_DIR = FONT_DIR / "vazirmatn"


def patch_fonts(metrics: dict[str, int]) -> None:
    OUT_DIR.mkdir(exist_ok=True)
    for src in sorted(ORIG_DIR.glob("Vazirmatn-*.ttf")):
        dst = OUT_DIR / src.name
        shutil.copy2(src, dst)
        font = TTFont(dst)
        os2 = font["OS/2"]
        hhea = font["hhea"]
        os2.sTypoAscender = metrics["typo_asc"]
        os2.sTypoDescender = metrics["typo_desc"]
        os2.sTypoLineGap = 0
        hhea.ascent = metrics["typo_asc"]
        hhea.descent = metrics["typo_desc"]
        hhea.lineGap = 0
        font.save(dst)
        print(f"  patched {dst.relative_to(REPO_ROOT)}")


def main() -> None:
    if not ORIG_DIR.exists() or not list(ORIG_DIR.glob("Vazirmatn-*.ttf")):
        raise SystemExit(
            f"no pristine Vazirmatn TTFs found under {ORIG_DIR.relative_to(REPO_ROOT)}; "
            "seed it manually from upstream before re-patching"
        )

    print(f"typoAsc={METRICS['typo_asc']}  typoDesc={METRICS['typo_desc']}")
    patch_fonts(METRICS)


if __name__ == "__main__":
    main()
