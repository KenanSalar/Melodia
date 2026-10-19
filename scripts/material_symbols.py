"""What the two icon scripts share about the Material Symbols faces.

`subset-icon-fonts.py` keeps a face's icons by resolving their names, and `check-icons.py` checks
the names it kept still resolve, so the two must agree on what resolving means. Importing this also
gates both on fontTools.
"""

from __future__ import annotations

import sys
from collections.abc import Callable

try:
    from fontTools.ttLib import TTFont
except ImportError:
    sys.exit(
        "ERROR: fontTools is not installed. Install it with one of:\n"
        "  python3 -m pip install --user fonttools\n"
        "  Fedora: sudo dnf install python3-fonttools\n"
        "  Debian/Ubuntu: sudo apt install python3-fonttools\n"
        "  Windows: py -m pip install fonttools"
    )

__all__ = ["TTFont", "character_glyphs", "ligature_resolver"]


def character_glyphs(font: TTFont) -> dict[str, str]:
    """Each character the face maps, to its glyph: the letters an icon name is spelled with."""
    cmap: dict[int, str] = {}
    for table in font["cmap"].tables:
        cmap.update(table.cmap)
    return {chr(codepoint): glyph for codepoint, glyph in cmap.items()}


def ligature_resolver(font: TTFont) -> Callable[[str], str | None]:
    """A function from an icon name to the glyph its ligature substitutes, None where none does.

    Material Symbols reaches each icon through a GSUB ligature: the text "play_arrow", a run of
    plain letter glyphs, is replaced by the icon's outline.
    """
    glyphs = character_glyphs(font)
    # First component glyph -> [(the remaining components, the ligature's glyph)].
    ligatures: dict[str, list[tuple[tuple[str, ...], str]]] = {}
    for lookup in font["GSUB"].table.LookupList.Lookup:
        for subtable in lookup.SubTable:
            if lookup.LookupType == 7:
                subtable = subtable.ExtSubTable
            for first, entries in (getattr(subtable, "ligatures", None) or {}).items():
                for ligature in entries:
                    ligatures.setdefault(first, []).append(
                        (tuple(ligature.Component), ligature.LigGlyph)
                    )

    def resolve(name: str) -> str | None:
        if not name or any(character not in glyphs for character in name):
            return None
        spelled = [glyphs[character] for character in name]
        for rest, glyph in ligatures.get(spelled[0], []):
            if rest == tuple(spelled[1:]):
                return glyph
        return None

    return resolve
