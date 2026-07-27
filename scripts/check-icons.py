#!/usr/bin/env python3
"""Drift guard for the subsetted Material Symbols faces.

The bundled icon TTFs under `melodia-ui/ui/assets/fonts/` are trimmed to exactly the icons in
`scripts/icons.txt` (see `subset-icon-fonts.sh`). An icon used in code but absent
from that list renders as a blank box. This script catches that drift:

  1. COVERAGE — every Material Symbols icon referenced in an icon context in the
     `.slint` sources is present in `scripts/icons.txt`.  (hard fail)
  2. INTEGRITY — every name in `scripts/icons.txt` resolves via ligature in BOTH
     the outlined and filled OUTPUT faces.                  (hard fail)
  3. ADVISORY — string literals that happen to be real icon names but live in
     non-icon contexts (view ids, dialog kinds, …) are listed, not failed.

"Is this string a real icon?" is answered against the full catalogue in
`scripts/fonts-src/` (the same pristine source the subset script consumes), so this
must run where that dir is populated. Exit code is non-zero on any hard failure.

Usage:  python3 scripts/check-icons.py
"""
import glob
import os
import re
import sys

from fontTools.ttLib import TTFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICONS_TXT = os.path.join(ROOT, "scripts", "icons.txt")
SRC = os.path.join(ROOT, "scripts", "fonts-src", "MaterialSymbolsRounded.ttf")
UI_DIR = os.path.join(ROOT, "melodia-ui", "ui")
OUT_OUTLINED = os.path.join(UI_DIR, "assets", "fonts", "MaterialSymbolsRounded.ttf")
OUT_FILLED = os.path.join(UI_DIR, "assets", "fonts", "MaterialSymbolsRoundedFilled.ttf")

TOKEN = re.compile(r'"([a-z][a-z0-9_]*)"')
# Sinks that carry an icon ligature name: MaterialIcon `name`, wrapper `icon`/
# `glyph`/`*-icon` props, raw `text:` on a Material-Symbols Text, and `return "x"`
# inside icon-name helper functions (e.g. volume-icon-name()).
SINK = re.compile(
    r'(?:\b(?:name|icon|glyph|text|leading-icon|trailing-icon|icon-name)\s*:'
    r'|\breturn)(.*?);',
    re.S,
)


def ligature_resolver(path):
    """Return f(name)->output-glyph-or-None for a Material Symbols face."""
    font = TTFont(path)
    cmap = {}
    for t in font["cmap"].tables:
        cmap.update(t.cmap)
    char2g = {chr(cp): gn for cp, gn in cmap.items()}
    idx = {}
    for lk in font["GSUB"].table.LookupList.Lookup:
        for st in lk.SubTable:
            if lk.LookupType == 7:
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

    return resolve


def main():
    if not os.path.isfile(SRC):
        sys.exit(
            "ERROR: %s missing.\nThis check needs the full catalogue. Populate "
            "scripts/fonts-src/ (see subset-icon-fonts.sh header)." % SRC
        )
    is_icon = ligature_resolver(SRC)
    listed = {l.strip() for l in open(ICONS_TXT) if l.strip()}

    # Gather every string literal, split by icon-context vs. anywhere.
    sources = glob.glob(os.path.join(UI_DIR, "**", "*.slint"), recursive=True)
    if not sources:
        sys.exit(f"ERROR: no .slint sources under {UI_DIR} — coverage would pass vacuously")
    in_sink, anywhere = set(), set()
    for fp in sources:
        txt = open(fp).read()
        anywhere.update(TOKEN.findall(txt))
        for m in SINK.finditer(txt):
            in_sink.update(TOKEN.findall(m.group(1)))

    used = {t for t in in_sink if is_icon(t)}          # real icons in icon contexts
    coincidental = {t for t in anywhere if is_icon(t)} - used  # real names, other ctx

    failures = []

    # 1. COVERAGE
    missing = sorted(used - listed)
    if missing:
        failures.append(
            "icons used in code but MISSING from scripts/icons.txt "
            "(would render as tofu): " + ", ".join(missing)
        )

    # 2. INTEGRITY
    for label, path in (("outlined", OUT_OUTLINED), ("filled", OUT_FILLED)):
        resolve = ligature_resolver(path)
        broken = sorted(n for n in listed if not resolve(n))
        if broken:
            failures.append(
                "names in icons.txt that do NOT resolve in the %s face %s: %s"
                % (label, os.path.basename(path), ", ".join(broken))
            )

    # 3. ADVISORY (never fails the build)
    stale = sorted(listed - used)
    if stale:
        print("note: in icons.txt but not seen in an icon context (stale?): "
              + ", ".join(stale))
    if coincidental:
        print("note: real icon names used in non-icon contexts (ids/kinds/labels), "
              "excluded by design: " + ", ".join(sorted(coincidental)))

    if failures:
        print("\nICON DRIFT — re-run scripts/subset-icon-fonts.sh after fixing:")
        for f in failures:
            print("  - " + f)
        sys.exit(1)

    print("OK: %d icons, all covered and resolving in both faces." % len(listed))


if __name__ == "__main__":
    main()
