#!/usr/bin/env python3
r"""Builds a synthetic music library for Melodia's scan and footprint tests.

Every track is a short tune composed from its own seed, tagged with colour and material
names and a real genre. An album's tracks share one flat cover, in a colour no other
album uses. The catalogue derives from one seed, so a rerun rebuilds the same library
and skips the files that already exist.

Needs ffmpeg on PATH and three Python packages:

    pip install numpy Pillow mutagen

It asks for a folder and builds the library in a folder of its own inside it, named after
the catalogue: melodia-test-library-50500 by default. --out answers that up front, and
pointing it at the library's own folder resumes a run there:

    python3 scripts/generate-test-library.py
    py scripts\generate-test-library.py --out D:\test-data
"""

from __future__ import annotations

import argparse
import base64
import functools
import io
import itertools
import math
import os
import random
import shutil
import subprocess
import sys
import time
from collections import Counter
from collections.abc import Iterator
from dataclasses import dataclass
from multiprocessing import Pool
from pathlib import Path
from typing import NamedTuple

import numpy as np
from mutagen.aiff import AIFF
from mutagen.flac import FLAC, Picture
from mutagen.id3 import APIC, ID3, TALB, TCMP, TCON, TDRC, TIT2, TPE1, TPE2, TPOS, TRCK
from mutagen.mp4 import MP4, MP4Cover
from mutagen.oggopus import OggOpus
from mutagen.oggvorbis import OggVorbis
from mutagen.wave import WAVE
from PIL import Image

DEFAULT_TOTAL = 50_500
DEFAULT_SEED = 50_500

# A track's whole length, tail included: inside 10 to 20 s, with room at both edges for
# what an encoder pads or trims. The window has to stay wider than the slowest bar, or
# some tempo has no bar count that fits.
MIN_SECONDS = 10.5
MAX_SECONDS = 19.8
TAIL_SECONDS = 1.5
FADE_IN_SECONDS = 0.01
FADE_OUT_SECONDS = 1.2

# MAX_PATH counts the terminating NUL.
WINDOWS_MAX_PATH = 260
# Piped output gets a line every this many tracks instead of the bar.
PROGRESS_EVERY = 500
BAR_MIN_WIDTH = 10
BAR_MAX_WIDTH = 40

EARLIEST_YEAR = 1965
LATEST_DEBUT = 2018
LATEST_YEAR = 2026
YEARS_BETWEEN_ALBUMS = (1, 4)
TRACKS_PER_ALBUM = (5, 16)
TRACKS_PER_COMPILATION = (10, 20)
VARIOUS_ARTISTS = "Various Artists"
COMPILATION_SHARE = 0.025
# A compilation credits artists already planned, so it waits until there are some.
COMPILATION_MIN_ARTISTS = 20
MULTI_DISC_SHARE = 0.04
MULTI_DISC_MIN_TRACKS = 10
TWO_GENRE_SHARE = 0.08
OFF_GENRE_SHARE = 0.2
FEATURING_SHARE = 0.03
SHARED_TITLE_SHARE = 0.01
PNG_COVER_SHARE = 0.15
PROGRESSIVE_JPEG_SHARE = 0.2
JPEG_QUALITY = (75, 95)
NON_SQUARE_COVER_SHARE = 0.08


def _words(text: str) -> list[str]:
    return list(dict.fromkeys(word.strip() for word in text.split(",") if word.strip()))


COLORS = _words("""
    Absinthe, Alabaster, Alizarin, Almond, Amaranth, Amber, Amethyst, Apple, Apricot, Aqua,
    Aquamarine, Arctic, Arylide, Ash, Asparagus, Aubergine, Auburn, Aureolin, Avocado, Azure,
    Banana, Beaver, Beige, Beryl, Bisque, Bistre, Bittersweet, Black, Blizzard, Blond, Blue,
    Blueberry, Bluebell, Blush, Bole, Bordeaux, Brick, Brown, Bubblegum, Buff, Burgundy,
    Burlywood, Burnt Orange, Butter, Byzantium, Cadet, Cadmium, Camel, Cameo, Canary, Candy,
    Capri, Caramel, Cardinal, Carmine, Carnation, Carolina, Carrot, Celadon, Celeste, Cerise,
    Cerulean, Champagne, Chartreuse, Cherry, Chestnut, Chocolate, Cinnabar, Cinnamon, Citron,
    Claret, Cocoa, Coffee, Coquelicot, Cornflower, Cornsilk, Cosmic, Cranberry, Cream, Crimson,
    Cyan, Cyclamen, Daffodil, Dandelion, Desert, Dove, Dusk, Ecru, Eggplant, Eggshell, Emerald,
    Eucalyptus, Fallow, Fawn, Feldgrau, Fern, Firebrick, Flame, Flamingo, Flax, Forest, Fuchsia,
    Fulvous, Gamboge, Ginger, Gingerbread, Glacier, Glaucous, Grape, Gray, Green, Grullo,
    Gunmetal, Harlequin, Hazel, Heather, Heliotrope, Hibiscus, Honey, Honeydew, Hunter, Iceberg,
    Inchworm, Indigo, Iris, Isabelline, Jasmine, Jazzberry, Jet, Jonquil, Juniper, Kelly,
    Keppel, Khaki, Kobe, Lagoon, Lapis, Lava, Lavender, Lemon, Licorice, Lilac, Lime, Liver,
    Lust, Magenta, Maize, Mandarin, Mango, Marigold, Maroon, Mauve, Melon, Midnight, Mikado,
    Mimosa, Mint, Moccasin, Moss, Mulberry, Mustard, Myrtle, Navy, Nectarine, Neon, Nutmeg,
    Ochre, Olive, Orange, Orange Peel, Orchid, Oxblood, Pansy, Papaya, Paprika, Pastel, Peach,
    Peacock, Pear, Pecan, Periwinkle, Persimmon, Phlox, Pink, Pistachio, Plum, Pomegranate,
    Poppy, Primrose, Puce, Pumpkin, Purple, Quince, Raspberry, Raven, Razzmatazz, Red,
    Rhubarb, Rose, Rosso, Russet, Rust, Sable, Saffron, Sage, Salmon, Sand, Sangria, Scarlet,
    Seafoam, Seashell, Sepia, Shamrock, Sienna, Sky, Smalt, Smoke, Snow, Sorrel, Spruce,
    Sunflower, Sunglow, Sunset, Tan, Tangerine, Taupe, Tawny, Tea Rose, Teal, Thistle, Thulian,
    Tiffany, Toffee, Tomato, Tulip, Tuscan, Twilight, Tyrian, Ultramarine, Umber, Vanilla,
    Verdigris, Vermilion, Violet, Viridian, Volt, Watermelon, Wheat, White, Wine, Wisteria,
    Xanadu, Xanthic, Yellow, Zaffre, Zomp
""")

MATERIALS = _words("""
    Acrylic, Adobe, Aerogel, Agate, Alloy, Alpaca, Aluminum, Andesite, Antimony, Asphalt,
    Bakelite, Balsa, Bamboo, Bark, Basalt, Beeswax, Beryllium, Birch, Bismuth, Bitumen,
    Bloodstone, Bone, Boxwood, Brass, Brocade, Bronze, Buckram, Burlap, Calico, Cambric,
    Canvas, Carbon, Cardboard, Carnelian, Cashmere, Cast Iron, Cedar, Celluloid, Cement,
    Ceramic, Chalk, Chambray, Charcoal, Chenille, Chert, Chiffon, Chrome, Cinder, Citrine,
    Clay, Coal, Cobalt, Cobblestone, Concrete, Copper, Coral, Corduroy, Cork, Cotton, Crepe,
    Crystal, Damask, Denim, Diamond, Dolomite, Driftwood, Ebony, Elm, Enamel, Felt,
    Fiberglass, Flannel, Fleece, Flint, Fluorite, Gabardine, Garnet, Gauze, Gilt, Glass,
    Glassine, Gneiss, Gold, Gossamer, Granite, Graphite, Gravel, Gypsum, Hematite, Hemp,
    Hickory, Horn, Ice, Iridium, Iron, Ivory, Jacquard, Jade, Jasper, Jersey, Jute, Kaolin,
    Kevlar, Lace, Lacquer, Lapis Lazuli, Larch, Latex, Lead, Leather, Lignite, Limestone,
    Linen, Lodestone, Loam, Lucite, Magnesium, Mahogany, Malachite, Maple, Marble, Marl,
    Mercury, Mesh, Mica, Mohair, Molybdenum, Moonstone, Mother of Pearl, Muslin, Nacre,
    Neoprene, Nickel, Nylon, Oak, Obsidian, Oilcloth, Onyx, Opal, Organza, Osmium, Palladium,
    Paper, Papyrus, Parchment, Pearl, Peat, Percale, Pewter, Pine, Plaster, Platinum,
    Plexiglass, Plywood, Polyester, Poplin, Porcelain, Porphyry, Pumice, Quartz, Raffia,
    Rattan, Rawhide, Rayon, Redwood, Resin, Rhinestone, Rhodium, Rope, Rosewood, Rubber, Ruby,
    Sandstone, Sapphire, Sateen, Satin, Scrim, Seersucker, Serge, Shale, Shell, Shellac,
    Silicon, Silk, Silver, Sinew, Sisal, Slate, Soapstone, Sodalite, Spandex, Sponge,
    Stainless Steel, Steel, Sterling, Stone, Straw, Stucco, Suede, Taffeta, Tantalum, Tarmac,
    Teak, Terracotta, Terrazzo, Tin, Tinfoil, Titanium, Topaz, Tourmaline, Travertine, Tuff,
    Tulle, Tungsten, Turquoise, Tweed, Twill, Vellum, Velour, Velvet, Veneer, Vinyl, Viscose,
    Voile, Walnut, Wax, Wicker, Willow, Wool, Wrought Iron, Yew, Zebrawood, Zinc, Zirconium
""")

ENSEMBLES = _words("""
    Collective, Ensemble, Orchestra, Quartet, Quintet, Trio, Duo, Society, Choir, Sound System,
    Project, Assembly, Syndicate, Club, Circle, Band, Parade, Revival, Experience, Foundation,
    Guild, Brothers, Sisters, Machine, Lights, Union, Commune, Workshop, Atelier, Express,
    Motel, Department, Institute, Archive, Caravan, Radio, Arcade, Engine
""")

ALBUM_NOUNS = _words("""
    Horizons, Sessions, Echoes, Signals, Tides, Seasons, Rooms, Letters, Stories, Waves, Lines,
    Fields, Dreams, Mirrors, Gardens, Voyages, Bridges, Windows, Hours, Shadows, Rivers, Maps,
    Engines, Postcards, Daydreams, Reflections, Patterns, Frequencies, Weather, Nights,
    Mornings, Currents, Atlas, Chronicles, Fragments, Sketches, Studies, Variations, Nocturnes,
    Anthems, Ballads, Dances, Harbours, Lanterns, Orbits, Satellites, Canyons, Islands
""")

SHARED_ALBUM_TITLES = ("Greatest Hits", "Live", "B-Sides", "Demos", "Rarities")

# Genre to the arrangement it is composed in; the genres themselves are the tag values.
GENRE_STYLES = {
    "Ambient": "calm", "New Age": "calm", "Classical": "calm", "Baroque": "calm",
    "Romantic": "calm", "Opera": "calm", "Minimalism": "calm", "Soundtrack": "calm",
    "Folk": "calm", "Celtic": "calm", "Singer-Songwriter": "calm", "Chillout": "calm",
    "Post-Rock": "calm", "Musical Theatre": "calm",
    "Downtempo": "beat", "Lo-Fi": "beat", "Trip Hop": "beat", "Hip-Hop": "beat",
    "Trap": "beat", "Grime": "beat", "Reggaeton": "beat", "Dub": "beat", "Dubstep": "beat",
    "IDM": "beat", "Vaporwave": "beat",
    "R&B": "groove", "Soul": "groove", "Funk": "groove", "Synthpop": "groove",
    "New Wave": "groove", "Pop": "groove", "J-Pop": "groove", "City Pop": "groove",
    "Latin Pop": "groove", "Reggae": "groove", "Shoegaze": "groove",
    "Psychedelic Rock": "groove", "Soft Rock": "groove", "Breakbeat": "groove",
    "Smooth Jazz": "groove", "Jazz Fusion": "groove", "Gospel": "groove", "Country": "groove",
    "Americana": "groove", "Bossa Nova": "groove", "Tango": "groove", "Flamenco": "groove",
    "Afrobeat": "groove", "Highlife": "groove", "Cumbia": "groove",
    "Disco": "dance", "House": "dance", "Techno": "dance", "Trance": "dance",
    "Eurodance": "dance", "Italo Disco": "dance", "Dance": "dance", "Electropop": "dance",
    "K-Pop": "dance", "Industrial": "dance", "Chiptune": "dance", "Samba": "dance",
    "Salsa": "dance",
    "Rock": "drive", "Hard Rock": "drive", "Heavy Metal": "drive", "Metal": "drive",
    "Punk": "drive", "Post-Punk": "drive", "Grunge": "drive", "Indie Rock": "drive",
    "Alternative Rock": "drive", "Garage Rock": "drive", "Progressive Rock": "drive",
    "Emo": "drive", "Hardcore": "drive", "Drum and Bass": "drive", "Ska": "drive",
    "Bluegrass": "drive", "Zydeco": "drive",
    "Jazz": "swing", "Bebop": "swing", "Cool Jazz": "swing", "Swing": "swing",
    "Blues": "swing", "Klezmer": "swing",
}
GENRES = list(GENRE_STYLES)

ALBUMS_PER_ARTIST = (1, 2, 3, 4, 5, 6, 8)
ALBUMS_PER_ARTIST_WEIGHTS = (25, 25, 18, 12, 8, 7, 5)

SQUARE_COVER_SIZES = {
    100: 1, 150: 1, 200: 3, 250: 2, 300: 6, 350: 2, 400: 5, 500: 10, 600: 10, 640: 4,
    700: 4, 750: 4, 800: 10, 900: 3, 1000: 10, 1080: 3, 1200: 8, 1400: 4, 1425: 2,
    1500: 6, 1600: 3, 1800: 2, 2000: 4, 2400: 2, 3000: 2, 3500: 1, 4000: 1,
}
NON_SQUARE_COVER_SIZES = (
    (640, 480), (800, 600), (1024, 768), (1280, 720), (1920, 1080), (600, 800), (900, 1200),
    (1000, 1100), (1200, 1000), (500, 375), (1600, 1200), (2048, 1536),
)


@dataclass(frozen=True)
class Encoding:
    label: str
    tag_family: str
    extension: str
    sample_rate: int
    codec_args: tuple[str, ...]
    muxer: str
    id3_version: int = 4


@dataclass(frozen=True)
class Cover:
    color: tuple[int, int, int]
    width: int
    height: int
    image_format: str
    quality: int
    progressive: bool

    @property
    def mime(self) -> str:
        return "image/png" if self.image_format == "PNG" else "image/jpeg"


@dataclass(frozen=True)
class TrackJob:
    path: str
    title: str
    artist: str
    album: str
    album_artist: str
    genres: tuple[str, ...]
    year: int
    track: int
    track_total: int
    disc: int
    disc_total: int
    compilation: bool
    encoding: Encoding
    style: str
    seed: int
    target_seconds: float
    cover: Cover


def _mp3(rng: random.Random) -> Encoding:
    rate = rng.choice(("128k", "192k", "256k", "320k", "V0", "V2"))
    quality = ("-q:a", rate[1]) if rate.startswith("V") else ("-b:a", rate)
    args = ("-c:a", "libmp3lame", *quality, "-id3v2_version", "0")
    return Encoding("mp3", "id3", "mp3", 44100, args, "mp3", rng.choice((3, 4)))


def _flac(rng: random.Random) -> Encoding:
    rate, bits = rng.choices(((44100, 16), (48000, 24), (96000, 24)), weights=(80, 15, 5))[0]
    depth = ("-sample_fmt", "s16") if bits == 16 else ("-sample_fmt", "s32", "-bits_per_raw_sample", "24")
    return Encoding("flac", "vorbis", "flac", rate, ("-c:a", "flac", *depth), "flac")


def _aac(rng: random.Random) -> Encoding:
    if rng.random() < 0.6:
        codec = ("-c:a", "aac", "-b:a", rng.choice(("128k", "192k", "256k")))
    else:
        codec = ("-c:a", "libfdk_aac", "-vbr", rng.choice(("3", "4", "5")))
    rate = rng.choice((44100, 48000))
    return Encoding("m4a/aac", "mp4", "m4a", rate, (*codec, "-movflags", "+faststart"), "ipod")


def _alac(rng: random.Random) -> Encoding:
    return Encoding("m4a/alac", "mp4", "m4a", 44100, ("-c:a", "alac", "-sample_fmt", "s16p"), "ipod")


def _vorbis(rng: random.Random) -> Encoding:
    extension = "oga" if rng.random() < 0.1 else "ogg"
    args = ("-c:a", "libvorbis", "-q:a", str(rng.randint(3, 7)))
    return Encoding(extension, "ogg", extension, 44100, args, "ogg")


def _opus(rng: random.Random) -> Encoding:
    args = ("-c:a", "libopus", "-b:a", rng.choice(("96k", "128k", "160k")))
    return Encoding("opus", "opus", "opus", 48000, args, "opus")


def _wav(rng: random.Random) -> Encoding:
    rate, codec = rng.choices(((44100, "pcm_s16le"), (48000, "pcm_s24le")), weights=(85, 15))[0]
    return Encoding("wav", "wave", "wav", rate, ("-c:a", codec), "wav")


def _aiff(rng: random.Random) -> Encoding:
    extension = "aif" if rng.random() < 0.25 else "aiff"
    rate, codec = rng.choices(((44100, "pcm_s16be"), (48000, "pcm_s24be")), weights=(85, 15))[0]
    return Encoding(extension, "aiff", extension, rate, ("-c:a", codec), "aiff")


ENCODERS = (_mp3, _flac, _aac, _alac, _vorbis, _opus, _wav, _aiff)
ENCODER_WEIGHTS = (38, 16, 13, 5, 11, 10, 4, 3)


def build_catalogue(total: int, seed: int, root: Path) -> list[TrackJob]:
    rng = random.Random(seed)
    titles = iter(unique_titles(rng, total))
    albums = plan_albums(rng, total)
    jobs: list[TrackJob] = []
    for album, color in zip(albums, unique_colors(rng, len(albums))):
        encoding = rng.choices(ENCODERS, weights=ENCODER_WEIGHTS)[0](rng)
        cover = pick_cover(rng, color)
        folder = root / safe_component(album.album_artist) / safe_component(f"{album.year} - {album.title}")
        discs = disc_layout(rng, len(album.credits))
        credits = iter(album.credits)
        for disc, disc_size in enumerate(discs, 1):
            for track in range(1, disc_size + 1):
                title = next(titles)
                prefix = f"{disc}-{track:02d}" if len(discs) > 1 else f"{track:02d}"
                jobs.append(TrackJob(
                    path=str(folder / safe_component(f"{prefix} - {title}.{encoding.extension}")),
                    title=title,
                    artist=next(credits),
                    album=album.title,
                    album_artist=album.album_artist,
                    genres=album.genres,
                    year=album.year,
                    track=track,
                    track_total=disc_size,
                    disc=disc,
                    disc_total=len(discs),
                    compilation=album.compilation,
                    encoding=encoding,
                    style=GENRE_STYLES[album.genres[0]],
                    seed=rng.getrandbits(32),
                    target_seconds=rng.uniform(MIN_SECONDS, MAX_SECONDS),
                    cover=cover,
                ))
    return jobs


def unique_titles(rng: random.Random, total: int) -> list[str]:
    pairs = list(dict.fromkeys(f"{c} {m}" for c in COLORS for m in MATERIALS if c != m))
    if len(pairs) < total:
        sys.exit(f"Only {len(pairs)} colour and material titles exist for {total} tracks.")
    rng.shuffle(pairs)
    return pairs[:total]


def unique_colors(rng: random.Random, total: int) -> list[tuple[int, int, int]]:
    """Points on an even RGB grid, spaced wide enough that JPEG's quantiser keeps them apart."""
    side = 2
    while side**3 < total:
        side += 1
    levels = [round(i * 255 / (side - 1)) for i in range(side)]
    colors = list(itertools.product(levels, repeat=3))
    rng.shuffle(colors)
    return colors[:total]


def pick_cover(rng: random.Random, color: tuple[int, int, int]) -> Cover:
    if rng.random() < NON_SQUARE_COVER_SHARE:
        width, height = rng.choice(NON_SQUARE_COVER_SIZES)
    else:
        width = height = rng.choices(list(SQUARE_COVER_SIZES), weights=list(SQUARE_COVER_SIZES.values()))[0]
    image_format = "PNG" if rng.random() < PNG_COVER_SHARE else "JPEG"
    quality = rng.randint(*JPEG_QUALITY)
    return Cover(color, width, height, image_format, quality, rng.random() < PROGRESSIVE_JPEG_SHARE)


def safe_component(name: str) -> str:
    """A path component every desktop filesystem accepts, Windows included."""
    cleaned = "".join("_" if ch in '<>:"/\\|?*' else ch for ch in name)
    return cleaned.rstrip(". ") or "_"


@dataclass
class AlbumPlan:
    album_artist: str
    title: str
    year: int
    genres: tuple[str, ...]
    compilation: bool
    credits: list[str]


def plan_albums(rng: random.Random, total: int) -> list[AlbumPlan]:
    """Albums holding exactly `total` tracks, the last one cut short to land on it."""
    albums: list[AlbumPlan] = []
    planned = 0
    for album in AlbumPlanner(rng).albums():
        albums.append(album)
        planned += len(album.credits)
        if planned >= total:
            break
    last = albums[-1]
    del last.credits[len(last.credits) - (planned - total):]
    return albums


class AlbumPlanner:
    """Plans albums one artist at a time, with compilations of the artists so far between them."""

    def __init__(self, rng: random.Random) -> None:
        self._rng = rng
        self._names = Names(rng)
        self._artists: list[str] = []

    def albums(self) -> Iterator[AlbumPlan]:
        while True:
            for album in self._career():
                yield album
                if len(self._artists) > COMPILATION_MIN_ARTISTS and self._rng.random() < COMPILATION_SHARE:
                    yield self._compilation()

    def _career(self) -> Iterator[AlbumPlan]:
        rng = self._rng
        artist = self._names.artist()
        self._artists.append(artist)
        home_genre = rng.choice(GENRES)
        year = rng.randint(EARLIEST_YEAR, LATEST_DEBUT)
        shared_titles = list(SHARED_ALBUM_TITLES)
        for _ in range(rng.choices(ALBUMS_PER_ARTIST, weights=ALBUMS_PER_ARTIST_WEIGHTS)[0]):
            if shared_titles and rng.random() < SHARED_TITLE_SHARE:
                title = shared_titles.pop(rng.randrange(len(shared_titles)))
            else:
                title = self._names.album()
            credits = [self._credit(artist) for _ in range(rng.randint(*TRACKS_PER_ALBUM))]
            yield AlbumPlan(artist, title, year, album_genres(rng, home_genre), False, credits)
            year = min(year + rng.randint(*YEARS_BETWEEN_ALBUMS), LATEST_YEAR)

    def _credit(self, artist: str) -> str:
        guests = self._artists[:-1]
        if guests and self._rng.random() < FEATURING_SHARE:
            return f"{artist} feat. {self._rng.choice(guests)}"
        return artist

    def _compilation(self) -> AlbumPlan:
        rng = self._rng
        credits = [rng.choice(self._artists) for _ in range(rng.randint(*TRACKS_PER_COMPILATION))]
        title = self._names.compilation()
        year = rng.randint(EARLIEST_YEAR, LATEST_YEAR)
        return AlbumPlan(VARIOUS_ARTISTS, title, year, (rng.choice(GENRES),), True, credits)


class Names:
    """Hands out artist and album names no earlier call has used."""

    def __init__(self, rng: random.Random) -> None:
        self._rng = rng
        self._used: set[str] = {VARIOUS_ARTISTS}

    def artist(self) -> str:
        rng = self._rng
        patterns = (
            lambda: f"{rng.choice(COLORS)} {rng.choice(ENSEMBLES)}",
            lambda: f"The {rng.choice(MATERIALS)} {rng.choice(ENSEMBLES)}",
            lambda: f"{rng.choice(COLORS)} & {rng.choice(MATERIALS)}",
        )
        return self._fresh(lambda: rng.choice(patterns)())

    def album(self) -> str:
        rng = self._rng
        patterns = (
            lambda: f"{rng.choice(MATERIALS)} {rng.choice(ALBUM_NOUNS)}",
            lambda: f"{rng.choice(ALBUM_NOUNS)} of {rng.choice(MATERIALS)}",
            lambda: f"The {rng.choice(COLORS)} {rng.choice(ALBUM_NOUNS)}",
        )
        return self._fresh(lambda: rng.choice(patterns)())

    def compilation(self) -> str:
        rng = self._rng
        return self._fresh(lambda: f"{rng.choice(COLORS)} {rng.choice(ALBUM_NOUNS)} Vol. {rng.randint(1, 9)}")

    def _fresh(self, build) -> str:
        while True:
            name = build()
            if name not in self._used:
                self._used.add(name)
                return name


def album_genres(rng: random.Random, home_genre: str) -> tuple[str, ...]:
    first = rng.choice(GENRES) if rng.random() < OFF_GENRE_SHARE else home_genre
    if rng.random() >= TWO_GENRE_SHARE:
        return (first,)
    return (first, rng.choice([genre for genre in GENRES if genre != first]))


def disc_layout(rng: random.Random, track_count: int) -> list[int]:
    if track_count >= MULTI_DISC_MIN_TRACKS and rng.random() < MULTI_DISC_SHARE:
        first = track_count // 2
        return [first, track_count - first]
    return [track_count]


TABLE_SIZE = 2048
VIBRATO_HZ = 5.5
BEATS_PER_BAR = 4
TONIC_RANGE = (55, 66)
MELODY_SPAN = 20
LEAD_GAIN = 0.3
LEAD_VELOCITY = (0.8, 1.0)
LEAD_PAN = (-0.3, 0.3)
LEGATO = 0.92
PAD_PANS = (-0.45, 0.0, 0.45, 0.2)
PAD_HOLD = 0.98
BASS_HOLD = 0.9
# A swung offbeat moves from the half beat to the last triplet.
SWING_DELAY_BEATS = 2 / 3 - 1 / 2
# A dotted eighth.
ECHO_BEATS = 0.75
ECHO_GAIN = 0.22
# Spread so ReplayGain has a range of levels to measure.
PEAK_DBFS = (-7.0, -1.0)
DRUM_KIT_SEED = 7


def _table(harmonics: tuple[float, ...]) -> np.ndarray:
    phase = np.arange(TABLE_SIZE) / TABLE_SIZE
    wave = sum(level * np.sin(2 * np.pi * (k + 1) * phase) for k, level in enumerate(harmonics) if level)
    return (wave / np.max(np.abs(wave))).astype(np.float32)


def midi_hz(pitch: float) -> float:
    return 440.0 * 2.0 ** ((pitch - 69) / 12)


@dataclass(frozen=True, eq=False)
class Voice:
    table: np.ndarray
    attack: float
    decay: float
    sustain: float
    release: float
    vibrato: float = 0.0

    def render(self, pitch: float, hold: float, sample_rate: int) -> np.ndarray:
        t = np.arange(int((hold + self.release) * sample_rate)) / sample_rate
        freq = midi_hz(pitch)
        cycles = freq * t
        if self.vibrato:
            # The phase is the integral of the wobbling frequency, so it never jumps.
            cycles += self.vibrato * freq * (1 - np.cos(2 * np.pi * VIBRATO_HZ * t)) / (2 * np.pi * VIBRATO_HZ)
        position = (cycles % 1.0) * TABLE_SIZE
        lower = position.astype(np.int32)
        upper = (lower + 1) % TABLE_SIZE
        frac = (position - lower).astype(np.float32)
        wave = self.table[lower] + (self.table[upper] - self.table[lower]) * frac
        return wave * self._envelope(t, hold)

    def _envelope(self, t: np.ndarray, hold: float) -> np.ndarray:
        attack = np.minimum(t / self.attack, 1.0)
        body = self.sustain + (1 - self.sustain) * np.exp(-np.maximum(t - self.attack, 0) / self.decay)
        release = np.clip(1 - (t - hold) / self.release, 0, 1)
        return (attack * body * release).astype(np.float32)


VOICES = {
    "flute": Voice(_table((1, 0.2, 0.06, 0.02)), 0.05, 0.4, 0.85, 0.12, vibrato=0.004),
    "clarinet": Voice(_table((1, 0, 0.45, 0, 0.25, 0, 0.12, 0, 0.06)), 0.03, 0.3, 0.8, 0.1, vibrato=0.003),
    "brass": Voice(_table(tuple(1 / k for k in range(1, 11))), 0.06, 0.25, 0.75, 0.1, vibrato=0.003),
    "square": Voice(_table(tuple(1 / k if k % 2 else 0 for k in range(1, 12))), 0.01, 0.2, 0.55, 0.06),
    "organ": Voice(_table((1, 0.7, 0, 0.45, 0, 0.3, 0, 0.2)), 0.01, 1.0, 1.0, 0.05),
    "epiano": Voice(_table((1, 0.45, 0.15, 0.08, 0.04)), 0.005, 0.5, 0.2, 0.25),
    "bass": Voice(_table((1, 0.55, 0.25, 0.1)), 0.01, 0.3, 0.6, 0.06),
    "pad": Voice(_table((1, 0.3, 0.2, 0.1, 0.05)), 0.35, 1.0, 1.0, 0.5),
}


@dataclass(frozen=True)
class Part:
    """An instrument's level and place in the stereo field, from -1 (left) to 1 (right)."""

    gain: float
    pan: float

    def channel_gains(self) -> tuple[float, float]:
        angle = (self.pan + 1) * math.pi / 4  # constant-power pan law
        return self.gain * math.cos(angle), self.gain * math.sin(angle)


BASS_PART = Part(0.34, 0.0)
DRUM_PARTS = {"kick": Part(0.55, 0.0), "snare": Part(0.3, 0.0), "hat": Part(0.14, 0.25)}

SCALES = (
    (0, 2, 4, 5, 7, 9, 11),  # major
    (0, 2, 3, 5, 7, 8, 10),  # natural minor
    (0, 2, 3, 5, 7, 9, 10),  # dorian
    (0, 2, 4, 5, 7, 9, 10),  # mixolydian
    (0, 2, 4, 6, 7, 9, 11),  # lydian
    (0, 2, 3, 5, 7, 8, 11),  # harmonic minor
)
# Scale-degree roots, one chord a bar; a phrase spans one pass.
PROGRESSIONS = (
    (0, 4, 5, 3), (0, 5, 3, 4), (5, 3, 0, 4), (1, 4, 0, 0),
    (0, 3, 4, 3), (0, 6, 5, 4), (0, 3, 0, 4), (0, 2, 3, 4),
)

BUSY_RHYTHMS = (
    (1, 1, 1, 1), (0.5, 0.5, 1, 1, 1), (1.5, 0.5, 1, 1), (1, 0.5, 0.5, 2), (0.5,) * 8,
    (0.75, 0.25, 1, 0.5, 0.5, 1), (1, 1, 0.5, 0.5, 1), (0.5, 1, 0.5, 2), (1.5, 1.5, 1),
)
RELAXED_RHYTHMS = ((2, 2), (1, 1, 2), (3, 1), (2, 1, 1), (1.5, 0.5, 2), (4,), (1, 1, 1, 1))
CADENCES = ((2, 2), (1, 1, 2), (3, 1), (1.5, 0.5, 2))
MELODIC_STEPS = (-3, -2, -1, -1, 0, 1, 1, 2, 3)
SEQUENCE_SHIFTS = (-2, -1, 1, 2)
REST_CHANCE = 0.07
PHRASE_FORM = "ABAC"

EIGHTHS = tuple(i / 2 for i in range(8))
OFFBEATS = tuple(i + 0.5 for i in range(4))
DRUM_PATTERNS = {
    "backbeat": {"kick": (0, 2, 2.5), "snare": (1, 3), "hat": EIGHTHS},
    "rock": {"kick": (0, 1.5, 2), "snare": (1, 3), "hat": EIGHTHS},
    "four": {"kick": (0, 1, 2, 3), "snare": (1, 3), "hat": OFFBEATS},
    "halftime": {"kick": (0, 1.75, 2.5), "snare": (2,), "hat": EIGHTHS},
    "ride": {"kick": (0, 2), "hat": (0, 1, 1 + 2 / 3, 2, 3, 3 + 2 / 3)},
}
CLOSING_HITS = {"kick": (0,)}

# (beat, length in beats, chord tone: 0 root, 1 third, 2 fifth, 3 octave)
BASS_PATTERNS = {
    "held": ((0, 4, 0),),
    "pulse": ((0, 1.5, 0), (1.5, 0.5, 0), (2, 1.5, 2), (3.5, 0.5, 0)),
    "eighths": tuple((i / 2, 0.5, 0) for i in range(8)),
    "offbeat": tuple((i + 0.5, 0.5, 0) for i in range(4)),
    "sparse": ((0, 1.5, 0), (2.5, 1, 0), (3.5, 0.5, 2)),
    "walking": ((0, 1, 0), (1, 1, 1), (2, 1, 2), (3, 1, 3)),
}


@dataclass(frozen=True)
class Style:
    bpm: tuple[int, int]
    drums: str | None
    leads: tuple[str, ...]
    bass: str
    pad_gain: float
    rhythms: tuple[tuple[float, ...], ...]
    chord_size: int = 3
    swing: bool = False


STYLES = {
    "calm": Style((60, 90), None, ("flute", "epiano", "clarinet"), "held", 0.12, RELAXED_RHYTHMS),
    "groove": Style((88, 118), "backbeat", ("epiano", "organ", "square", "clarinet"), "pulse", 0.07, BUSY_RHYTHMS),
    "drive": Style((128, 172), "rock", ("brass", "square"), "eighths", 0.06, BUSY_RHYTHMS),
    "dance": Style((118, 132), "four", ("square", "brass", "organ"), "offbeat", 0.08, BUSY_RHYTHMS),
    "beat": Style((70, 96), "halftime", ("epiano", "flute", "square"), "sparse", 0.08, BUSY_RHYTHMS),
    "swing": Style((96, 160), "ride", ("clarinet", "brass", "epiano"), "walking", 0.05, BUSY_RHYTHMS, 4, True),
}


@dataclass(frozen=True)
class Key:
    tonic: int
    scale: tuple[int, ...]

    def chord(self, degree: int, size: int, octave: int = 0) -> list[int]:
        """The chord stacked in thirds on `degree`, `octave` octaves from the tonic's."""
        steps = [degree + 2 * k for k in range(size)]
        return [self.tonic + 12 * (octave + step // 7) + self.scale[step % 7] for step in steps]

    def pitch_classes(self, degree: int) -> set[int]:
        return {pitch % 12 for pitch in self.chord(degree, 3)}

    def notes(self, span: int) -> list[int]:
        """The scale from the tonic up `span` semitones."""
        return [pitch for pitch in range(self.tonic, self.tonic + span) if (pitch - self.tonic) % 12 in self.scale]


class Event(NamedTuple):
    beat: float
    length: float
    pitch_index: int


class Melodist:
    """Walks a melody across bars, keeping its place so one phrase runs into the next."""

    def __init__(self, rng: random.Random, notes: list[int]) -> None:
        self._rng = rng
        self._notes = notes
        self._index = rng.randrange(len(notes) // 3, 2 * len(notes) // 3)

    def pitch(self, event: Event) -> int:
        return self._notes[event.pitch_index]

    def phrase(self, rhythms, chords: list[set[int]]) -> list[list[Event]]:
        """Four bars: an idea, its answer, the idea moved a step, and a cadence."""
        idea = self._bar(self._rng.choice(rhythms), chords[0])
        answer = self._bar(self._rng.choice(rhythms), chords[1])
        shift = self._rng.choice(SEQUENCE_SHIFTS)
        moved = [
            self._on_chord(event._replace(pitch_index=self._clamp(event.pitch_index + shift)), chords[2])
            for event in idea
        ]
        cadence = self._bar(self._rng.choice(CADENCES), chords[3])
        cadence[-1] = self._to_chord_tone(cadence[-1], chords[3])
        return [idea, answer, moved, cadence]

    def home(self, after: Event) -> list[Event]:
        """A bar held on the tonic nearest where the melody left off."""
        tonic_class = self._notes[0] % 12
        tonics = [i for i, pitch in enumerate(self._notes) if pitch % 12 == tonic_class]
        return [Event(0, BEATS_PER_BAR, min(tonics, key=lambda i: abs(i - after.pitch_index)))]

    def _bar(self, rhythm, chord: set[int]) -> list[Event]:
        events = []
        for beat, length in zip(itertools.accumulate(rhythm, initial=0.0), rhythm):
            if beat > 0 and self._rng.random() < REST_CHANCE:
                continue
            step = self._clamp(self._index + self._rng.choice(MELODIC_STEPS))
            event = self._on_chord(Event(beat, length, step), chord)
            self._index = event.pitch_index
            events.append(event)
        return events

    def _on_chord(self, event: Event, chord: set[int]) -> Event:
        """Pulls a note on beat 1 or 3 onto the chord; the rest may pass through."""
        if event.beat % 2 != 0:
            return event
        return self._to_chord_tone(event, chord)

    def _to_chord_tone(self, event: Event, chord: set[int]) -> Event:
        index = event.pitch_index
        for distance in range(len(self._notes)):
            for candidate in (index - distance, index + distance):
                if 0 <= candidate < len(self._notes) and self._notes[candidate] % 12 in chord:
                    return event._replace(pitch_index=candidate)
        return event

    def _clamp(self, index: int) -> int:
        return min(max(index, 0), len(self._notes) - 1)


class Mixer:
    """A stereo buffer every part is summed into."""

    def __init__(self, seconds: float, sample_rate: int) -> None:
        self.sample_rate = sample_rate
        self._buffer = np.zeros((int(seconds * sample_rate), 2), dtype=np.float32)

    def add(self, start: float, wave: np.ndarray, part: Part) -> None:
        first = int(start * self.sample_rate)
        count = min(len(wave), len(self._buffer) - first)
        if count <= 0:
            return
        left, right = part.channel_gains()
        self._buffer[first:first + count, 0] += wave[:count] * left
        self._buffer[first:first + count, 1] += wave[:count] * right

    def master(self, echo_delay: float, peak: float) -> np.ndarray:
        """Adds a ping-pong echo, scales the mix to `peak` and fades both ends."""
        buffer = self._buffer
        delay = int(echo_delay * self.sample_rate)
        dry = buffer.copy()
        buffer[delay:, 0] += ECHO_GAIN * dry[:-delay, 1]
        buffer[delay:, 1] += ECHO_GAIN * dry[:-delay, 0]
        buffer *= peak / float(np.max(np.abs(buffer)))
        fade_in = int(FADE_IN_SECONDS * self.sample_rate)
        buffer[:fade_in] *= np.linspace(0, 1, fade_in, dtype=np.float32)[:, None]
        fade_out = int(FADE_OUT_SECONDS * self.sample_rate)
        buffer[-fade_out:] *= np.linspace(1, 0, fade_out, dtype=np.float32)[:, None]
        return buffer


@functools.cache
def drum_kit(sample_rate: int) -> dict[str, np.ndarray]:
    noise = np.random.default_rng(DRUM_KIT_SEED)

    def span(seconds: float) -> np.ndarray:
        return np.arange(int(seconds * sample_rate)) / sample_rate

    t = span(0.3)
    kick = np.sin(2 * np.pi * np.cumsum(50 + 90 * np.exp(-t / 0.03)) / sample_rate) * np.exp(-t / 0.13)
    t = span(0.2)
    snare = 0.65 * noise.uniform(-1, 1, len(t)) * np.exp(-t / 0.07) + 0.5 * np.sin(2 * np.pi * 185 * t) * np.exp(-t / 0.04)
    t = span(0.08)
    hat = np.diff(noise.uniform(-1, 1, len(t) + 1)) * np.exp(-t / 0.02) * 0.5
    return {name: wave.astype(np.float32) for name, wave in (("kick", kick), ("snare", snare), ("hat", hat))}


def fit_bars(target: float, bar: float) -> int:
    """The bar count whose track length lands closest to `target` inside the length bounds."""
    fitting = [
        count for count in range(1, math.floor(MAX_SECONDS / bar) + 1)
        if MIN_SECONDS <= count * bar + TAIL_SECONDS <= MAX_SECONDS
    ]
    return min(fitting, key=lambda count: abs(count * bar + TAIL_SECONDS - target))


class Song:
    """One track's arrangement, composed and mixed from the track's seed."""

    def __init__(self, job: TrackJob) -> None:
        self._rng = random.Random(job.seed)
        self._style = STYLES[job.style]
        self._beat = 60 / self._rng.uniform(*self._style.bpm)
        self._bar = BEATS_PER_BAR * self._beat
        self._bar_count = fit_bars(job.target_seconds, self._bar)
        self._mixer = Mixer(self._bar_count * self._bar + TAIL_SECONDS, job.encoding.sample_rate)
        self._key = Key(self._rng.randint(*TONIC_RANGE), self._rng.choice(SCALES))
        self._progression = self._rng.choice(PROGRESSIONS)

    def render(self) -> np.ndarray:
        self._play_lead()
        self._play_pad()
        self._play_bass()
        if self._style.drums:
            self._play_drums()
        peak = 10 ** (self._rng.uniform(*PEAK_DBFS) / 20)
        return self._mixer.master(ECHO_BEATS * self._beat, peak)

    def _at(self, bar: int, beat: float) -> float:
        return bar * self._bar + beat * self._beat

    def _is_last(self, bar: int) -> bool:
        return bar == self._bar_count - 1

    def _degree(self, bar: int) -> int:
        """The chord under `bar`; the last bar resolves home."""
        return 0 if self._is_last(bar) else self._progression[bar % len(self._progression)]

    def _compose(self, melodist: Melodist) -> list[list[Event]]:
        chords = [self._key.pitch_classes(degree) for degree in self._progression]
        phrases: dict[str, list[list[Event]]] = {}
        bars: list[list[Event]] = []
        for group in range(math.ceil(self._bar_count / len(self._progression))):
            letter = PHRASE_FORM[group % len(PHRASE_FORM)]
            if letter not in phrases:
                phrases[letter] = melodist.phrase(self._style.rhythms, chords)
            bars.extend(phrases[letter])
        bars = bars[: self._bar_count]
        bars[-1] = melodist.home(bars[-1][-1])
        return bars

    def _play_lead(self) -> None:
        melodist = Melodist(self._rng, self._key.notes(MELODY_SPAN))
        voice = VOICES[self._rng.choice(self._style.leads)]
        pan = self._rng.uniform(*LEAD_PAN)
        for bar, events in enumerate(self._compose(melodist)):
            for event in events:
                start = self._at(bar, event.beat)
                if self._style.swing and event.beat % 1 == 0.5:
                    start += SWING_DELAY_BEATS * self._beat
                wave = voice.render(melodist.pitch(event), event.length * self._beat * LEGATO, self._mixer.sample_rate)
                self._mixer.add(start, wave, Part(LEAD_GAIN * self._rng.uniform(*LEAD_VELOCITY), pan))

    def _play_pad(self) -> None:
        for bar in range(self._bar_count):
            chord = self._key.chord(self._degree(bar), self._style.chord_size, octave=-1)
            for pitch, pan in zip(chord, PAD_PANS):
                wave = VOICES["pad"].render(pitch, self._bar * PAD_HOLD, self._mixer.sample_rate)
                self._mixer.add(self._at(bar, 0), wave, Part(self._style.pad_gain, pan))

    def _play_bass(self) -> None:
        for bar in range(self._bar_count):
            root, third, fifth = self._key.chord(self._degree(bar), 3, octave=-2)
            tones = (root, third, fifth, root + 12)
            pattern = BASS_PATTERNS["held" if self._is_last(bar) else self._style.bass]
            for beat, length, tone in pattern:
                wave = VOICES["bass"].render(tones[tone], length * self._beat * BASS_HOLD, self._mixer.sample_rate)
                self._mixer.add(self._at(bar, beat), wave, BASS_PART)

    def _play_drums(self) -> None:
        kit = drum_kit(self._mixer.sample_rate)
        for bar in range(self._bar_count):
            hits = CLOSING_HITS if self._is_last(bar) else DRUM_PATTERNS[self._style.drums]
            for drum, beats in hits.items():
                for beat in beats:
                    self._mixer.add(self._at(bar, beat), kit[drum], DRUM_PARTS[drum])


def cover_bytes(cover: Cover) -> bytes:
    image = Image.new("RGB", (cover.width, cover.height), cover.color)
    buffer = io.BytesIO()
    if cover.image_format == "PNG":
        image.save(buffer, "PNG")
    else:
        image.save(buffer, "JPEG", quality=cover.quality, progressive=cover.progressive)
    return buffer.getvalue()


def encode(pcm: np.ndarray, encoding: Encoding, destination: Path) -> None:
    command = [
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
        "-f", "f32le", "-ar", str(encoding.sample_rate), "-ch_layout", "stereo", "-i", "pipe:0",
        "-map_metadata", "-1", "-fflags", "+bitexact", "-flags:a", "+bitexact", "-threads", "1",
        *encoding.codec_args, "-f", encoding.muxer, str(destination),
    ]
    subprocess.run(command, input=pcm.astype("<f4").tobytes(), check=True, capture_output=True)


def id3_frames(job: TrackJob, art: bytes) -> list:
    frames = [
        TIT2(encoding=3, text=job.title),
        TPE1(encoding=3, text=job.artist),
        TPE2(encoding=3, text=job.album_artist),
        TALB(encoding=3, text=job.album),
        TCON(encoding=3, text=list(job.genres)),
        TDRC(encoding=3, text=str(job.year)),
        TRCK(encoding=3, text=f"{job.track}/{job.track_total}"),
        TPOS(encoding=3, text=f"{job.disc}/{job.disc_total}"),
        APIC(encoding=3, mime=job.cover.mime, type=3, desc="Cover", data=art),
    ]
    if job.compilation:
        frames.append(TCMP(encoding=3, text="1"))
    return frames


def vorbis_fields(job: TrackJob) -> dict[str, list[str]]:
    fields = {
        "TITLE": [job.title],
        "ARTIST": [job.artist],
        "ALBUMARTIST": [job.album_artist],
        "ALBUM": [job.album],
        "GENRE": list(job.genres),
        "DATE": [str(job.year)],
        "TRACKNUMBER": [str(job.track)],
        "TRACKTOTAL": [str(job.track_total)],
        "DISCNUMBER": [str(job.disc)],
        "DISCTOTAL": [str(job.disc_total)],
    }
    if job.compilation:
        fields["COMPILATION"] = ["1"]
    return fields


def flac_picture(cover: Cover, art: bytes) -> Picture:
    picture = Picture()
    picture.type = 3
    picture.mime = cover.mime
    picture.width = cover.width
    picture.height = cover.height
    picture.depth = 24
    picture.data = art
    return picture


def tag_id3(path: Path, job: TrackJob, art: bytes) -> None:
    tags = ID3()
    for frame in id3_frames(job, art):
        tags.add(frame)
    if job.encoding.id3_version == 3:
        tags.update_to_v23()
    tags.save(path, v2_version=job.encoding.id3_version)


def tag_chunked(container):
    def write(path: Path, job: TrackJob, art: bytes) -> None:
        audio = container(path)
        audio.add_tags()
        for frame in id3_frames(job, art):
            audio.tags.add(frame)
        audio.save()
    return write


def tag_flac(path: Path, job: TrackJob, art: bytes) -> None:
    audio = FLAC(path)
    if audio.tags is None:
        audio.add_tags()
    for key, values in vorbis_fields(job).items():
        audio[key] = values
    audio.add_picture(flac_picture(job.cover, art))
    audio.save()


def tag_ogg(container):
    def write(path: Path, job: TrackJob, art: bytes) -> None:
        audio = container(path)
        for key, values in vorbis_fields(job).items():
            audio[key] = values
        block = flac_picture(job.cover, art).write()
        audio["METADATA_BLOCK_PICTURE"] = [base64.b64encode(block).decode("ascii")]
        audio.save()
    return write


def tag_mp4(path: Path, job: TrackJob, art: bytes) -> None:
    audio = MP4(path)
    if audio.tags is None:
        audio.add_tags()
    image_format = MP4Cover.FORMAT_PNG if job.cover.image_format == "PNG" else MP4Cover.FORMAT_JPEG
    tags = audio.tags
    tags["\xa9nam"] = [job.title]
    tags["\xa9ART"] = [job.artist]
    tags["aART"] = [job.album_artist]
    tags["\xa9alb"] = [job.album]
    tags["\xa9gen"] = list(job.genres)
    tags["\xa9day"] = [str(job.year)]
    tags["trkn"] = [(job.track, job.track_total)]
    tags["disk"] = [(job.disc, job.disc_total)]
    tags["covr"] = [MP4Cover(art, imageformat=image_format)]
    if job.compilation:
        tags["cpil"] = True
    audio.save()


TAG_WRITERS = {
    "id3": tag_id3,
    "wave": tag_chunked(WAVE),
    "aiff": tag_chunked(AIFF),
    "vorbis": tag_flac,
    "ogg": tag_ogg(OggVorbis),
    "opus": tag_ogg(OggOpus),
    "mp4": tag_mp4,
}


@dataclass(frozen=True)
class Outcome:
    label: str
    size: int
    skipped: bool = False
    error: str | None = None


def partial_path(final: Path) -> Path:
    return final.with_name(f".{final.name}.part")


def render_track(job: TrackJob) -> Outcome:
    final = Path(job.path)
    label = job.encoding.label
    if final.exists():
        return Outcome(label, final.stat().st_size, skipped=True)
    partial = partial_path(final)
    try:
        final.parent.mkdir(parents=True, exist_ok=True)
        encode(Song(job).render(), job.encoding, partial)
        TAG_WRITERS[job.encoding.tag_family](partial, job, cover_bytes(job.cover))
        os.replace(partial, final)
    except subprocess.CalledProcessError as error:
        partial.unlink(missing_ok=True)
        return Outcome(label, 0, error=f"{final}: ffmpeg: {error.stderr.decode(errors='replace').strip()}")
    except Exception as error:  # one bad track is reported, not allowed to end the run
        partial.unlink(missing_ok=True)
        return Outcome(label, 0, error=f"{final}: {error!r}")
    return Outcome(label, final.stat().st_size)


def to_folder(raw: str) -> Path:
    """Reads a folder the way it gets pasted: quoted, or starting with ~ or a variable."""
    cleaned = raw.strip().strip("\"'")
    if not cleaned:
        sys.exit("No folder given.")
    return Path(os.path.expandvars(os.path.expanduser(cleaned))).resolve()


def ask_for_folder(library_name: str) -> Path:
    try:
        return to_folder(input(f"Folder to create {library_name} in: "))
    except EOFError:
        sys.exit("No folder given; pass --out when running without a terminal.")


def library_folder_name(total: int, seed: int) -> str:
    """Names the folder after what builds the catalogue, so two catalogues never share one."""
    name = f"melodia-test-library-{total}"
    return name if seed == DEFAULT_SEED else f"{name}-seed-{seed}"


def library_root(folder: Path, library_name: str) -> Path:
    """The library's own folder inside `folder`, unless `folder` already is it."""
    return folder if folder.name == library_name else folder / library_name


def windows_long_paths_enabled() -> bool:
    import winreg

    try:
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\FileSystem") as key:
            return winreg.QueryValueEx(key, "LongPathsEnabled")[0] == 1
    except OSError:  # no value is the default, which is off
        return False


def check_windows_path_lengths(jobs: list[TrackJob]) -> None:
    if sys.platform != "win32" or windows_long_paths_enabled():
        return
    longest = max(len(str(partial_path(Path(job.path)))) for job in jobs)
    if longest >= WINDOWS_MAX_PATH:
        sys.exit(
            f"The longest file path would be {longest} characters, past Windows' limit of "
            f"{WINDOWS_MAX_PATH - 1}. Pick a shorter folder, or turn on long paths "
            "(LongPathsEnabled) and run this again."
        )


def generate(jobs: list[TrackJob], workers: int) -> None:
    sizes: Counter[str] = Counter()
    counts: Counter[str] = Counter()
    skipped = 0
    failures: list[str] = []
    progress = Progress(len(jobs))
    progress.show()
    with Pool(workers) as pool:
        for outcome in pool.imap_unordered(render_track, jobs, chunksize=8):
            if outcome.error:
                failures.append(outcome.error)
                progress.note(f"FAILED {outcome.error}")
            else:
                sizes[outcome.label] += outcome.size
                counts[outcome.label] += 1
                skipped += outcome.skipped
            progress.advance(outcome)
    progress.finish()

    print(f"\ndone in {format_duration(progress.elapsed())}; "
          f"{skipped} already existed, {len(failures)} failed")
    for label, count in counts.most_common():
        print(f"  {label:9} {count:6}  {sizes[label] / 2**30:6.2f} GiB  avg {sizes[label] / count / 2**10:7.0f} KiB")
    print(f"  {'total':9} {sum(counts.values()):6}  {sum(sizes.values()) / 2**30:6.2f} GiB")
    if failures:
        sys.exit(1)


class Progress:
    """How far a run has got: a bar redrawn in place on a terminal, a line now and then when piped."""

    def __init__(self, total: int) -> None:
        self._total = total
        self._done = 0
        self._rendered = 0
        self._started = time.monotonic()
        self._live = sys.stdout.isatty()

    def elapsed(self) -> float:
        return time.monotonic() - self._started

    def advance(self, outcome: Outcome) -> None:
        self._done += 1
        self._rendered += not outcome.skipped
        self.show()

    def show(self) -> None:
        if self._live:
            self._draw()
        elif self._done % PROGRESS_EVERY == 0 or self._done == self._total:
            print(self._status(), flush=True)

    def note(self, message: str) -> None:
        """Prints a line above the bar; the next update draws the bar again under it."""
        if self._live:
            sys.stdout.write("\r" + " " * self._columns() + "\r")
        print(message, flush=True)

    def finish(self) -> None:
        if self._live:
            print(flush=True)

    def _status(self) -> str:
        elapsed = self.elapsed()
        # A skipped track finishes instantly, so only rendered ones set the pace.
        rate = self._rendered / elapsed if elapsed > 0 else 0.0
        eta = format_duration((self._total - self._done) / rate) if rate else "--"
        percent = 100 * self._done / self._total
        return (f"{self._done}/{self._total}  {percent:5.1f}%  eta {eta}"
                f"  {rate:.1f}/s  elapsed {format_duration(elapsed)}")

    def _draw(self) -> None:
        status = self._status()
        columns = self._columns()
        width = min(BAR_MAX_WIDTH, columns - len(status) - 3)
        if width < BAR_MIN_WIDTH:
            line = status
        else:
            filled = width * self._done // self._total
            line = f"[{'#' * filled}{'.' * (width - filled)}] {status}"
        sys.stdout.write("\r" + line[:columns].ljust(columns))
        sys.stdout.flush()

    @staticmethod
    def _columns() -> int:
        # A column short of the edge, where some consoles wrap; and the line is padded rather
        # than erased, since a legacy Windows console prints an erase sequence literally.
        return shutil.get_terminal_size().columns - 1


def format_duration(seconds: float) -> str:
    minutes, seconds = divmod(int(seconds), 60)
    hours, minutes = divmod(minutes, 60)
    return f"{hours}h{minutes:02d}m{seconds:02d}s" if hours else f"{minutes}m{seconds:02d}s"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", help="folder to create the library's folder in; asked for when left out")
    parser.add_argument("--total", type=int, default=DEFAULT_TOTAL, help="tracks to generate (default %(default)s)")
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED,
                        help="catalogue seed; the same seed rebuilds the same library (default %(default)s)")
    parser.add_argument("--workers", type=int, default=os.cpu_count() or 1,
                        help="tracks rendered in parallel (default: one per CPU)")
    parser.add_argument("--sample", type=int, help="render only this many tracks, spread across the catalogue")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if shutil.which("ffmpeg") is None:
        sys.exit("ffmpeg isn't on PATH; install it and run this again.")
    library_name = library_folder_name(args.total, args.seed)
    folder = to_folder(args.out) if args.out is not None else ask_for_folder(library_name)
    if folder.exists() and not folder.is_dir():
        sys.exit(f"{folder} is a file, not a folder.")
    root = library_root(folder, library_name)

    jobs = build_catalogue(args.total, args.seed, root)
    if args.sample:
        jobs = jobs[:: max(1, len(jobs) // args.sample)][: args.sample]
    check_windows_path_lengths(jobs)
    root.mkdir(parents=True, exist_ok=True)

    album_count = len({Path(job.path).parent for job in jobs})
    print(f"{len(jobs)} tracks in {album_count} albums -> {root}", flush=True)
    generate(jobs, args.workers)


if __name__ == "__main__":
    main()
