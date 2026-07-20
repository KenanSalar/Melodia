# Melodia

**A fast, lightweight cross-platform desktop music player built with [Slint](https://slint.dev/) and pure Rust.**

Melodia is a Slint rewrite of a former Tauri + SolidJS application — moving off the embedded WebKitGTK browser engine cut the real-world footprint from a combined **~900 MB** down to **below 150 MiB on Fedora**, with no IPC layer and no web runtime.

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL%20v3-blue.svg)](LICENSE)
[![Version](https://img.shields.io/github/v/release/KenanSalar/Melodia?label=version&color=blueviolet)](https://github.com/KenanSalar/Melodia/releases)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%C2%B7%20Windows-success.svg)](#platforms)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust%20%2B%20Slint-orange.svg)](https://www.rust-lang.org/)

---

## Screenshots

<!-- TODO: add screenshots. Drop image files into docs/screenshots/ and uncomment the links below. -->
<!--
![Library view](docs/screenshots/library.png)
![Now Playing](docs/screenshots/now-playing.png)
![Album detail](docs/screenshots/album-detail.png)
![Themes](docs/screenshots/themes.png)
-->

_Screenshots coming soon._

---

## Features

### Library Management
- Automatic scanning of music folders with parallel metadata extraction (Rayon)
- First-launch auto-detection of the OS Music directory
- Real-time folder watching with debounced re-scanning and incremental updates
- Content-hash-based moved-file detection (BLAKE3) that preserves play counts, favorites, and queue state when files are renamed or relocated
- Full-text search across tracks, albums, and artists (SQLite FTS5) with a top-result card, entity rows, and persistent recent search history
- Browse by albums, artists, genres, or the file system
- Dedicated detail pages for albums, artists, genres, and playlists
- Deezer-backed artist image fetching with local caching
- Favorites view with a hero header, artwork mosaic, most-played section, and a filterable track list
- Recently-Played view listing the tracks you last listened to (newest first), with a most-played strip and a filterable track list that updates live as you play
- Play-count and skip-count tracking per track
- Per-track star ratings (0–5), set inline via a hover-revealed star control in any track list and from the Now Playing view
- Edit track information (title, artist, album, album artist, genre, year, track/disc number, composer, comment, BPM, lyrics, and cover art) for one or many selected tracks at once, written straight back to the files — batch edits leave differing fields untouched and save only the fields you change
- Natural sort ordering for file and track names
- Customizable, resizable, and toggleable track-list columns
- Playlist creation and management with custom thumbnails
- Smart (dynamic) playlists whose membership is defined by rules rather than a fixed track list — match **all** or **any** of a set of conditions across fields like genre, artist, rating, year, play count, favorite, or when a track was last played/added, with an optional size cap and ordering (e.g. "50 most-played" or "top-rated"); membership is resolved live, so a smart playlist keeps itself up to date as your library and listening change
- Import and export playlists as standard `.m3u8` files (with embedded BLAKE3 content hashes) so they survive a database reset and interoperate with other players
- Drag-and-drop file import to playlists and the queue
- Drag-and-drop track reordering in playlists and the play queue
- Automatic pre-migration database backups

### Playback
- Gapless playback with a 2-deep Rodio queue
- Audio crossfade (1–12 s) that overlaps the end of one track with the start of the next, running the two on separate mixer decks with a sample-accurate complementary ramp so the sum can never clip; optionally skips same-album transitions to keep continuous mixes gapless, extends to manual track changes, and fades out on pause and stop
- Queue management with shuffle and repeat modes (Off, All, One)
- Full-screen Now Playing view with track details, an up-next list, and album-art cross-fade transitions
- 10-band graphic equalizer (31 Hz – 16 kHz) with adjustable preamp, nine built-in presets plus hand-tuned custom curves, and a soft-knee clip-protection limiter so boosts compress instead of clipping
- ReplayGain loudness normalization — applies per-track or per-album gain from the file's loudness tags (Track or Album mode), with an adjustable preamp and optional peak-based clip prevention; reuses the equalizer's soft-knee limiter so a boosted track compresses instead of clipping, and works with the equalizer off
- Playback speed control (0.25× – 2.0×)
- Sleep timer that pauses playback after a preset (15–90 min) or custom duration, or at the end of the current track; the duration countdown is playback-linked, so pausing the music holds the timer
- Volume control (0–100%) with mute
- Resume playback on startup
- OS media-key support
- Configurable play-button animation (none, ripple, or animated equalizer bars)
- Customizable player bar — relocate secondary controls into a compact overflow menu
- Responsive mini-player — shrinking the window past a threshold collapses the full UI into a compact horizontal strip or a square widget (the square grows an up-next list when tall enough); restore the full window from the mini-player's expand button

### Themes
Six theme families, each with light and dark variants and configurable accent colors:
- **Catppuccin** (Latte, Frappé, Macchiato, Mocha)
- **Material 3**
- **GNOME Adwaita**
- **KDE Breeze**
- **Windows Fluent**
- **macOS**

Automatic system dark/light mode detection is supported. Material You dynamic theming derives a palette from the current track's artwork, with selectable color styles (Tonal Spot, Vibrant, Expressive, Fidelity, Content, Monochrome, Neutral). On KDE, color schemes are read from `kdeglobals` for native palette integration, and the sidebar and now-playing bar can be tinted toward the content background when the window loses focus. A custom transparent titlebar is used by default — with an adjustable window corner radius — alongside an option to fall back to native window decorations.

### Supported Audio Formats
MP3, FLAC, M4A/AAC, OGG/Vorbis, WAV, ALAC, AIFF

### Internationalization
- 7 locales: English, German, French, Spanish, Turkish, Greek, Italian
- Slint-native `@tr()` translations with bundled `.po` files compiled into the binary
- Runtime locale switching with no restart

### System Integration
- OS media controls (Linux: MPRIS2, Windows: SMTC)
- Always-on-top support (Linux: KDE via KWin D-Bus, GNOME via shell extension)
- Window state persistence (size, position, maximized)
- Queue and navigation state persistence across sessions
- Signed auto-updater (minisign) with in-app install and toast notifications
- Self-deploying desktop entry and icon for tarball installs
- AppStream metadata so the app appears with full name, icon, and license in KDE Discover and GNOME Software

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Play / Pause | `Space` |
| Seek backward / forward 5s | `←` / `→` |
| Seek backward / forward 30s | `Shift+←` / `Shift+→` |
| Previous / Next track | `Ctrl+←` / `Ctrl+→`, or `P` / `N` |
| Volume up / down 5% | `↑` / `↓` |
| Volume up / down 1% | `Ctrl+↑` / `Ctrl+↓` |
| Seek to 0–90% | `0`–`9` |
| Mute | `M` |
| Favorite current track | `L` |
| Shuffle | `S` |
| Repeat mode | `R` |
| Queue sheet | `Q` |
| Now Playing view | `F` |
| Maximize | `F11` |
| Toggle sidebar | `Ctrl+B` |
| Settings | `Ctrl+,` |
| New playlist | `Ctrl+N` |
| Close dialog / Now Playing | `Esc` |
| Navigate back / forward through history | `Mouse-4` / `Mouse-5` |

OS media keys (play/pause, next, previous, stop) are also handled.

## Platforms

- Linux (X11 and Wayland)
- Windows

Pre-built releases ship for both `x86_64` and `aarch64`.

## Installation

Download the latest release for your platform from the
[Releases page](https://github.com/KenanSalar/Melodia/releases).

### Linux

| Format | Install |
|--------|---------|
| `.rpm` (Fedora/RHEL) | `sudo dnf install ./melodia-*.rpm` |
| `.deb` (Debian/Ubuntu) | `sudo apt install ./melodia-*.deb` |
| AppImage | `chmod +x Melodia-*.AppImage && ./Melodia-*.AppImage` |
| Tarball | Extract, then run `./install-linux.sh` (no `sudo` — installs into `~/.local/share/Melodia`) |

The tarball install is fully user-local, so the in-app updater works without a polkit prompt.

### Windows

Download and run the installer, or use the portable binary.

### Updates

Melodia ships a built-in, minisign-signed auto-updater that can download and install
new releases in place. Release artifacts also carry build-provenance attestations,
verifiable with:

```bash
gh attestation verify <file> --repo KenanSalar/Melodia
```

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs/) — **1.97.0**, edition 2024 (pinned by `rust-toolchain.toml`; rustup installs it automatically)

**Linux** — development packages for Slint's FemtoVG renderer (no WebKitGTK required):

```bash
# Debian/Ubuntu
sudo apt install libfontconfig1-dev libfreetype6-dev libasound2-dev \
                 libxkbcommon-dev mesa-vulkan-drivers libwayland-dev

# Fedora
sudo dnf install fontconfig-devel freetype-devel alsa-lib-devel \
                 libxkbcommon-devel vulkan-loader wayland-devel
```

**macOS / Windows** — no extra dependencies.

### Build

```bash
git clone https://github.com/KenanSalar/Melodia.git
cd Melodia

cargo run                                      # debug build, runs the app
cargo build --release                          # release build → target/release/Melodia
cargo clippy --all-targets -- -D warnings       # lint
cargo test                                      # run tests
```

> **Note — vendored winit fork.**
> The `winit/` directory is a checked-in copy of winit 0.30.13 plus an unmerged
> Wayland file drag-and-drop fix ([winit#1881]); `Cargo.toml`'s
> `[patch.crates-io]` block points at it. A fresh clone builds with no setup —
> the fork ships in the repo. Removing the `[patch.crates-io]` block (and
> `winit/`) drops Wayland file-manager drag-and-drop; everything else still
> builds.
>
> [winit#1881]: https://github.com/rust-windowing/winit/issues/1881

## Tech Stack

| Layer | Technology |
|-------|-----------|
| UI | [Slint](https://slint.dev/) 1.16 (FemtoVG renderer) |
| Backend | Pure Rust — direct calls + tokio channels, no IPC |
| Async runtime | [Tokio](https://tokio.rs/) |
| Audio | [Rodio](https://github.com/RustAudio/rodio) + [Symphonia](https://github.com/pdeljanov/Symphonia) |
| Equalizer DSP | [biquad](https://crates.io/crates/biquad) (peaking-filter bands) |
| Media Controls | [Souvlaki](https://github.com/Sinono3/souvlaki) (MPRIS2, SMTC) |
| Metadata | [Lofty](https://github.com/Serial-ATA/lofty-rs) |
| File Hashing | [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) |
| Parallelism | [Rayon](https://github.com/rayon-rs/rayon) |
| File Watching | [notify](https://github.com/notify-rs/notify) + notify-debouncer-full |
| Database | SQLite via [SQLx](https://github.com/launchbadge/sqlx) with FTS5 |
| Dynamic Color | [material-colors](https://crates.io/crates/material-colors) |
| Auto-updater | [minisign-verify](https://crates.io/crates/minisign-verify) |
| Windowing | [winit](https://github.com/rust-windowing/winit) (vendored fork for Wayland DnD) |

## Architecture

Melodia runs the Slint UI on the main thread and a single multi-threaded Tokio
runtime for all backend work (database, scanner, file watcher, player, HTTP).
There is no WebView and no IPC boundary:

- **UI → backend** — Slint callbacks spawn `async` work on the Tokio runtime.
- **Backend → UI** — state flows back over `tokio::sync::watch` / `mpsc`
  channels, consumed by UI-thread tasks that update Slint properties.

```
src/
├── boot/        startup sequencing
├── database/    SQLx + SQLite (WAL, FTS5, migrations)
├── entities/    domain model types (track, album, artist, genre, playlist, …)
├── library/     playback, queue, tracks, albums, artists, genres, playlists, search, settings
├── media/       scanner, metadata, artwork, cover-thumbnail cache, folder watcher
├── player/      playback state machine + dual-deck Rodio backend + graphic equalizer, ReplayGain & crossfade DSP
├── tasks/       background tasks (playback monitor, file events, queue prune, Material You)
├── themes/      pluggable theme registry
├── services/    updater, desktop integration, system theme
├── state/       AppState, error types
└── ui/          Slint bridge, callbacks, view handles, models
```

See [`CLAUDE.md`](CLAUDE.md) for a detailed architecture reference.

## Configuration & Data

Melodia stores its data under the OS application-data directory
(`~/.local/share/Melodia` on Linux, `%APPDATA%\Melodia` on Windows):

| File / folder | Purpose |
|---------------|---------|
| `melodia.db` | SQLite music library (WAL + FTS5) |
| `settings.json` | App/user preferences (theme, locale, playback, window geometry) |
| `views.json` | Per-view UI state (column widths, sort, browse path, open detail) |
| `queue.json` | Persisted play queue |
| `search_history.json` | Recent search terms (capped at 10) |
| `artwork/`, `artists/` | Cached album and artist images |

## Contributing

Contributions are welcome. Before opening a pull request:

- Run `cargo clippy --all-targets -- -D warnings` — the lint configuration is
  strict and `unwrap()` is denied in non-test code.
- Run `cargo test` and keep it green.
- Follow the existing [Conventional Commits](https://www.conventionalcommits.org/)
  style used in the git history.
- Open pull requests against the `main` branch.

Every pull request runs the **PR Validation** workflow — `clippy` (with
`-D warnings`) and the full test suite under coverage — and the
`pr-validation` check must pass before merging. Test coverage from the latest
run is published to GitHub Pages at
[kenansalar.github.io/Melodia](https://kenansalar.github.io/Melodia/).

## License

Melodia is licensed under the
[GNU Affero General Public License v3.0](LICENSE).

## Acknowledgments

Built on the work of the [Slint](https://slint.dev/),
[Rodio](https://github.com/RustAudio/rodio),
[Symphonia](https://github.com/pdeljanov/Symphonia), and
[SQLx](https://github.com/launchbadge/sqlx) projects, the
[Catppuccin](https://catppuccin.com/) palette, and
[Material Foundation](https://m3.material.io/)'s color utilities — along with
the many other crates listed in `Cargo.toml`.
