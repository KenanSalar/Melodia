#!/usr/bin/env python3
r"""Regenerates the Discord Rich Presence art in `assets/discord/`.

    python3 scripts/gen-discord-assets.py
    py scripts\gen-discord-assets.py

The images are uploaded by hand to the Discord Developer Portal. The icon and cover derive from the
logo SVGs; the play and pause badges are drawn here and have no other source.

    app-icon.png  General Information > App Icon, and the Rich Presence art key "melodia"
    cover.png     Rich Presence > Cover Image (16:9)
    playing.png   the Rich Presence art key "playing"
    paused.png    the Rich Presence art key "paused"

The keys are `payload.rs`'s ASSET_* constants, lowercase because Discord lowercases a key on upload.

Needs Inkscape and ImageMagick 7 on PATH.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOGO_TILE = ROOT / "assets/icons/logo-with-background.svg"
LOGO_MARK = ROOT / "crates/melodia-ui/ui/assets/icons/logo-without-background.svg"
OUT = ROOT / "assets/discord"

ICON_SIZE = 1024  # the canvas the badges' coordinates are drawn on
COVER_WIDTH, COVER_HEIGHT = 1024, 576

BACKGROUND = "#1e1e2e"  # Catppuccin Mocha's base
# The logo's `ml-grad` stops: recolouring the logo means recolouring these.
GRADIENT_START = "#89b4fa"
GRADIENT_END = "#94e2d5"


class Failure(Exception):
    """Ends the run with its message on stderr rather than a traceback."""


@dataclass(frozen=True)
class Tools:
    inkscape: str
    magick: str
    scratch: Path


@dataclass(frozen=True)
class Badge:
    name: str
    # ImageMagick options that paint the shape on a black canvas, the fill already white.
    drawing: tuple[str, ...]
    # Opposite corners of the shape's box, so the gradient's whole range lands on it.
    corners: tuple[str, str]


BADGES = (
    Badge(
        "playing",
        # The wide, round-joined stroke is what rounds the triangle's corners.
        (
            "-stroke", "white", "-strokewidth", "64",
            "-draw", "stroke-linejoin round polygon 400,320 400,704 730,512",
        ),
        ("400,320", "730,704"),
    ),
    Badge(
        "paused",
        (
            "-draw", "roundrectangle 312,296 456,728 48,48",
            "-draw", "roundrectangle 568,296 712,728 48,48",
        ),
        ("312,296", "712,728"),
    ),
)


def main() -> int:
    try:
        inkscape = find_tool("inkscape", "Inkscape")
        magick = find_tool("magick", "ImageMagick 7")
        OUT.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="melodia-discord-") as scratch:
            tools = Tools(inkscape, magick, Path(scratch))
            written = [paint_app_icon(tools), paint_cover(tools)]
            written += [paint_badge(tools, badge) for badge in BADGES]

        print(f"Wrote Discord assets to {OUT}:")
        for path in written:
            size = run(magick, "identify", "-format", "%wx%h", path)
            print(f"  {path.name:<12} {size}")
    except Failure as failure:
        print(f"error: {failure}", file=sys.stderr)
        return 1
    return 0


def find_tool(name: str, package: str) -> str:
    found = shutil.which(name)
    if found is None:
        raise Failure(f"{name} not found on PATH; install {package}")
    return found


def paint_app_icon(tools: Tools) -> Path:
    """Flattens the logo tile onto a full square, which Discord rounds itself."""
    tile = rasterize(tools, LOGO_TILE, ICON_SIZE, ICON_SIZE)
    out = OUT / "app-icon.png"
    run(
        tools.magick,
        "-size", f"{ICON_SIZE}x{ICON_SIZE}", f"xc:{BACKGROUND}",
        tile, "-gravity", "center", "-composite", out,
    )
    return out


def paint_cover(tools: Tools) -> Path:
    logo = rasterize(tools, LOGO_MARK, COVER_HEIGHT, COVER_HEIGHT)
    out = OUT / "cover.png"
    run(
        tools.magick,
        "-size", f"{COVER_WIDTH}x{COVER_HEIGHT}", f"xc:{BACKGROUND}",
        logo, "-gravity", "center", "-composite", out,
    )
    return out


def paint_badge(tools: Tools, badge: Badge) -> Path:
    """Fills the badge's shape with the logo's gradient, over the background."""
    canvas = f"{ICON_SIZE}x{ICON_SIZE}"
    mask = tools.scratch / f"{badge.name}-mask.png"
    gradient = tools.scratch / f"{badge.name}-gradient.png"
    shape = tools.scratch / f"{badge.name}-shape.png"
    out = OUT / f"{badge.name}.png"

    run(tools.magick, "-size", canvas, "xc:black", "-fill", "white", *badge.drawing, mask)
    start, end = badge.corners
    run(
        tools.magick,
        "-size", canvas, "xc:",
        "-sparse-color", "barycentric", f"{start} {GRADIENT_START} {end} {GRADIENT_END}",
        gradient,
    )
    run(
        tools.magick,
        gradient, "(", mask, "-alpha", "off", ")",
        "-compose", "CopyOpacity", "-composite", shape,
    )
    run(
        tools.magick,
        "-size", canvas, f"xc:{BACKGROUND}",
        shape, "-compose", "over", "-composite", out,
    )
    return out


def rasterize(tools: Tools, svg: Path, width: int, height: int) -> Path:
    png = tools.scratch / f"{svg.stem}.png"
    run(
        tools.inkscape, svg,
        "--export-type=png", "--export-area-page", f"--export-filename={png}",
        "-w", str(width), "-h", str(height),
    )
    return png


def run(*command: str | Path) -> str:
    """Returns `command`'s stdout. Its stderr surfaces only on failure, Inkscape's being noise."""
    answer = subprocess.run(
        [str(part) for part in command],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        errors="replace",
    )
    if answer.returncode != 0:
        raise Failure(f"{Path(command[0]).stem} failed: {answer.stderr.strip()}")
    return answer.stdout


if __name__ == "__main__":
    sys.exit(main())
