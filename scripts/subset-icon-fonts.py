#!/usr/bin/env python3
r"""Subsets the two Material Symbols Rounded faces down to the icon ligatures Melodia renders.

    python3 scripts/subset-icon-fonts.py
    py scripts\subset-icon-fonts.py

Both faces are trimmed: the outlined one (FILL=0) and the filled one (FILL=1).

Why this is by glyph id
    Each icon is reached through a ligature spelled from the plain letters a-z and the underscore.
    Subsetting by text (`--text=<every name>`) feeds those letters into pyftsubset's layout
    closure, and since every one of the ~3,600 icon names is spelled from them, the closure adds
    every icon back: only the variable tables ever shrank. So this resolves each name in
    `scripts/icons.txt` to its ligature's glyph, keeps those plus the letters, retains the
    liga/calt/dlig lookups so "play_arrow" still shapes at runtime, and turns the closure off.

`scripts/icons.txt` is load-bearing: an icon used in code but missing from it renders as a blank
box. `scripts/check-icons.py` re-derives the used set from the source tree and fails on drift.

Sources
    Read from `scripts/fonts-src/` (gitignored, several MiB each) and written to
    `crates/melodia-ui/ui/assets/fonts/`, never over their own input, so a face can always be
    subset again after adding an icon. Name the sources as the outputs:

        scripts/fonts-src/MaterialSymbolsRounded.ttf
        scripts/fonts-src/MaterialSymbolsRoundedFilled.ttf

    One upstream variable `MaterialSymbolsRounded[FILL,GRAD,opsz,wght].ttf`
    (google/material-design-icons) can supply both, copied under each name: a variable source is
    instanced at FILL=0 for the outlined face and FILL=1 for the filled one. A static source skips
    the instancer.

Needs fontTools.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

# Ahead of fontTools, so a missing fontTools stops with how to install it.
from material_symbols import TTFont, character_glyphs, ligature_resolver
from fontTools import subset
from fontTools.varLib import instancer

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "scripts" / "icons.txt"
SOURCES = ROOT / "scripts" / "fonts-src"
FONTS = ROOT / "crates" / "melodia-ui" / "ui" / "assets" / "fonts"

# Each face and the FILL it is instanced at from a variable source.
FACES = (("MaterialSymbolsRounded.ttf", 0), ("MaterialSymbolsRoundedFilled.ttf", 1))
# Material Symbols' default axes, which a static instance is cut at.
DEFAULT_AXES = ("wght=400", "GRAD=0", "opsz=24")


def main() -> None:
    if not ICONS.is_file():
        sys.exit(f"ERROR: {ICONS} not found.")
    names = [line.strip() for line in ICONS.read_text(encoding="utf-8").splitlines() if line.strip()]
    with tempfile.TemporaryDirectory(prefix="melodia-icons-") as scratch:
        for face, fill in FACES:
            subset_face(face, fill, names, Path(scratch))
    print("\nDone. Re-run `cargo build --release -p melodia` to embed the new subsets.")


def subset_face(face: str, fill: int, names: list[str], scratch: Path) -> None:
    source = SOURCES / face
    output = FONTS / face
    print(f"==> {face} (FILL={fill})")
    if not source.is_file():
        sys.exit(f"ERROR: source {source} missing. Populate scripts/fonts-src/ (see the docstring).")

    staged = source
    # The variable tables are what make the upstream font ~14 MiB, and instancing drops them.
    if "fvar" in TTFont(source):
        staged = scratch / face
        instancer.main(
            [str(source), f"--output={staged}", "--no-recalc-timestamp", f"FILL={fill}", *DEFAULT_AXES]
        )

    font = TTFont(staged)
    resolve = ligature_resolver(font)
    missing = [name for name in names if resolve(name) is None]
    if missing:
        sys.exit(f"ERROR: {staged} has no ligature for: {', '.join(missing)}")
    # Upstream also maps every icon to a private-use codepoint, and keeping those would keep the
    # whole catalogue.
    letters = {glyph for character, glyph in character_glyphs(font).items() if not is_private_use(character)}
    keep = letters | {resolve(name) for name in names}
    glyph_ids = sorted({font.getGlyphID(glyph) for glyph in keep})
    print(f"    keep {len(glyph_ids)} glyphs (source had {font['maxp'].numGlyphs})")

    # Exactly the glyphs above: liga/calt/dlig stay so the kept ligatures still fire, and the
    # closure that would re-expand to the whole catalogue stays off.
    subset.main(
        [
            str(staged),
            f"--output-file={output}",
            "--gids=" + ",".join(map(str, glyph_ids)),
            "--no-layout-closure",
            "--layout-features+=liga,calt,dlig",
            "--name-IDs=*",
            "--notdef-outline",
            "--no-hinting",
            "--drop-tables+=DSIG",
        ]
    )

    before = source.stat().st_size
    after = output.stat().st_size
    print(f"    {face:<40} {before:9d} -> {after:9d} bytes ({after / before * 100:.1f}% of source)")


def is_private_use(character: str) -> bool:
    codepoint = ord(character)
    return 0xE000 <= codepoint <= 0xF8FF or codepoint >= 0xF0000


if __name__ == "__main__":
    main()
