#!/usr/bin/env python3
"""Build the bundled Vazirmatn faces: the Greek, Russian, Ukrainian and
Vietnamese letters it lacks merged in from Roboto, then the line metrics
rewritten.

Slint takes one named family, and a character that family lacks is drawn from
an OS fallback whose taller line box throws off the centring the metric patch
below exists for. So every language the UI is expected to set has to live
inside these faces rather than beside them.

What a language needs is CLDR's answer, not a Unicode block's: its main
exemplar characters, in both cases, plus the combining marks their decomposed
spellings use, so text that arrives decomposed (a macOS file name) still maps
here. A block brings symbols, historic letters and the alphabets of languages
nobody asked for. Supporting another language is a row in `LANGUAGE_LETTERS`.

The added glyphs come from Roboto 3.004 because Vazirmatn's own Latin was
imported from that release (its name table says so). Instanced at each face's
weight it matches Vazirmatn's Latin advance for advance, so the new letters
share its stems and spacing. Any other release is refused.

Vazirmatn goes into the merge first and Roboto is cut down to the codepoints
Vazirmatn does not map, so the merge never has a duplicate to resolve and
nothing Vazirmatn already draws changes. The cost is kerning across the seam:
each font's GPOS names only its own glyphs, so a Vietnamese vowel from Roboto
does not kern against a capital from Vazirmatn (`Tạ`). Greek and Cyrillic come
from Roboto whole and kern among themselves.

Upstream Vazirmatn ships with OS/2 typo + hhea ascent/descent sized to clear
Arabic combining marks (ratio ~1.56x font-size). At the app's UI sizes that
shifts every vertically-centred Latin text upward by several px because the
typo line box is much taller than typical Latin UI fonts (~1.20x).

The metric patch rewrites only the layout-metric fields. Glyph outlines,
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
  originals/Vazirmatn-*.ttf             `fonts/ttf/` of upstream's vazirmatn-v33.003.zip
  originals/Roboto[ital,wdth,wght].ttf  `unhinted/` of upstream's Roboto_v3.004.zip
  vazirmatn/Vazirmatn-*.ttf             merged and patched, imported directly by the tree's app-window.slint

Neither original is committed -- re-download to update.

The script is idempotent: re-running always reads from originals/ and writes
the same patched outputs.

Usage:
    python3 scripts/patch_vazirmatn.py
"""

import tempfile
import unicodedata
from pathlib import Path

from fontTools import subset
from fontTools.merge import Merger
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

METRICS = {"typo_asc": 1650, "typo_desc": -500}

# CLDR 48's main exemplar characters, verbatim. The auxiliary sets would add only polytonic
# Greek, which the language dropped in 1982, and Vazirmatn already has every punctuation set.
LANGUAGE_LETTERS = {
    "el": "αάβγδεέζηήθιίϊΐκλμνξοόπρσςτυύϋΰφχψωώ",
    "ru": "абвгдеёжзийклмнопрстуфхцчшщъыьэюя",
    "uk": "абвгґдеєжзиіїйклмнопрстуфхцчшщьюяʼ",
    "vi": "aàảãáạăằẳẵắặâầẩẫấậbcdđeèẻẽéẹêềểễếệghiìỉĩíịklmnoòỏõóọôồổỗốộơờởỡớợpqrstuùủũúụưừửữứựvxyỳỷỹýỵ",
}

ROBOTO_VERSION = "Version 3.004"

REPO_ROOT = Path(__file__).resolve().parents[1]
FONT_DIR = REPO_ROOT / "crates" / "melodia-ui" / "ui" / "assets" / "fonts"
ORIG_DIR = FONT_DIR / "originals"
OUT_DIR = FONT_DIR / "vazirmatn"
ROBOTO_SOURCE = ORIG_DIR / "Roboto[ital,wdth,wght].ttf"


def load_roboto() -> TTFont:
    roboto = TTFont(ROBOTO_SOURCE)
    version = roboto["name"].getDebugName(5)
    if version != ROBOTO_VERSION:
        raise SystemExit(
            f"{ROBOTO_SOURCE.name} is {version!r}, not {ROBOTO_VERSION!r}; "
            "only that release is known to match Vazirmatn's Latin"
        )
    return roboto


def language_codepoints() -> set[int]:
    codepoints = set()
    for letters in LANGUAGE_LETTERS.values():
        for letter in letters:
            # `upper()` can return a sequence: ΐ has no precomposed capital.
            for spelling in (letter, letter.upper()):
                codepoints.update(map(ord, spelling))
                codepoints.update(map(ord, unicodedata.normalize("NFD", spelling)))
    return codepoints


def codepoints_to_import(vazirmatn: TTFont, roboto: TTFont) -> list[int]:
    wanted = language_codepoints() - vazirmatn.getBestCmap().keys()
    absent = wanted - roboto.getBestCmap().keys()
    if absent:
        raise SystemExit(
            f"Roboto has no glyph for {''.join(map(chr, sorted(absent)))!r}; "
            "a language it can't set needs another source"
        )
    return sorted(wanted)


def roboto_additions(roboto: TTFont, weight: int, codepoints: list[int]) -> TTFont:
    static = instancer.instantiateVariableFont(roboto, {"ital": 0, "wdth": 100, "wght": weight})

    # Default layout features: kerning, marks and the rest the shaper applies unasked,
    # without the small caps and stylistic sets nothing in the UI turns on.
    options = subset.Options()
    options.glyph_names = True
    options.drop_tables += ["STAT"]
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=codepoints)
    subsetter.subset(static)
    return static


def merge(vazirmatn_path: Path, additions: TTFont) -> TTFont:
    with tempfile.TemporaryDirectory() as scratch:
        additions_path = Path(scratch) / "roboto-additions.ttf"
        additions.save(additions_path)
        return Merger().merge([str(vazirmatn_path), str(additions_path)])


def patch_metrics(font: TTFont, metrics: dict[str, int]) -> None:
    os2 = font["OS/2"]
    hhea = font["hhea"]
    os2.sTypoAscender = metrics["typo_asc"]
    os2.sTypoDescender = metrics["typo_desc"]
    os2.sTypoLineGap = 0
    hhea.ascent = metrics["typo_asc"]
    hhea.descent = metrics["typo_desc"]
    hhea.lineGap = 0


def build_face(src: Path, roboto: TTFont) -> None:
    vazirmatn = TTFont(src)
    weight = vazirmatn["OS/2"].usWeightClass
    codepoints = codepoints_to_import(vazirmatn, roboto)
    additions = roboto_additions(roboto, weight, codepoints)

    merged = merge(src, additions)
    patch_metrics(merged, METRICS)

    # The merge stamps both dates with the current time; upstream's keep a re-run with
    # unchanged inputs from rewriting three committed binaries.
    merged["head"].created = vazirmatn["head"].created
    merged["head"].modified = vazirmatn["head"].modified
    merged.recalcTimestamp = False

    dst = OUT_DIR / src.name
    merged.save(dst)
    glyphs = merged["maxp"].numGlyphs - vazirmatn["maxp"].numGlyphs
    print(
        f"  wrote {dst.relative_to(REPO_ROOT)}  "
        f"(wght {weight}, {len(codepoints)} characters in {glyphs} glyphs from Roboto)"
    )


def main() -> None:
    faces = sorted(ORIG_DIR.glob("Vazirmatn-*.ttf"))
    if not faces or not ROBOTO_SOURCE.exists():
        raise SystemExit(
            f"pristine Vazirmatn and Roboto missing under {ORIG_DIR.relative_to(REPO_ROOT)}; "
            "seed it from upstream as the docstring describes before re-patching"
        )

    print(f"typoAsc={METRICS['typo_asc']}  typoDesc={METRICS['typo_desc']}")
    OUT_DIR.mkdir(exist_ok=True)
    roboto = load_roboto()
    for src in faces:
        build_face(src, roboto)


if __name__ == "__main__":
    main()
