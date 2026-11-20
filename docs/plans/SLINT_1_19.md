# Slint 1.18.1 → 1.19.0

Status: phase 1 done, gated and hand-tested; phase 2 next · Created: 2026-11-20

`slint` and `slint-build` move to 1.19.0, the latest release, and the local `slint-tr-extractor`
moves with them, `scripts/update-translations.py` refusing a mismatch. Phase 1 is the bump and the
facts it falsifies; phase 2 opts into edition 1.19, which applies the `if`/`for` sizing fixes the
pitfalls file works around. Every claim below was read from the 1.18.1 and 1.19.0 crate sources in
the cargo registry.

## Go/no-go

A toolkit bump is budgeted, not assumed (ADR 0003): 1.17 skipped enter-transitions and cost frame
time through the per-frame `ensure_tree_instantiated` walk in `draw_contents`, and was reverted.
1.19 still runs that walk, now over overlays too (`i-slint-core-1.19.0/window.rs:861-873`). Phase 1
stands only if the hand test finds the enter transitions playing and the queue sheet and nav
transitions as smooth as on 1.18.1. **Passed 2026-11-20**: transitions and the app behave as on
1.18.1.

## What reaches Melodia

### Nothing breaks

The breaking changes are C++, JS, Python, Skia Vulkan and the experimental
features variable, none of which we use. MSRV 1.95 sits under our 1.100. Every feature we enable
still exists and `slint-build` still requires `compat-1-18`. With no edition set the compiler keeps
1.18 language behaviour (`i-slint-compiler-1.19.0/lib.rs:399`) and `slint-build` drops the info
saying so (`slint-build-1.19.0/lib.rs:566`). No test reads the generated `app-window.rs`, and the
new warnings match nothing in the `.slint` tree.

### What changes at the default edition

- **Caret scrolling**: a focused `TextInput` scrolls every ancestor `Flickable` just far enough to
  show the caret, on each cursor move (`items/text.rs:2132-2136`, `item_tree.rs:1171-1194`). A field
  inside a settings or dialog scroller now scrolls its page. `MultilineInput`'s own 12 px margin
  handler runs first and stays, as upstream's `TextEdit` keeps its own.
- **Layout measurement**: a repeated cell of a layout measures at its real width (upstream
  c923cab2), a single-cell layout subtracts its side padding (8b87c803), and a fixed-size child
  with its own layout info passes its preferred size and stretch up (80a488e5).
- **Gradients interpolate premultiplied in FemtoVG**
  (`i-slint-renderer-femtovg-1.19.0/itemrenderer.rs:1737-1739`).
  Our gradients are same-RGB ramps or opaque. An eased brush still mixes through `Color::mix`,
  which weights RGB by alpha (`graphics/color.rs:384`), so a fade to `transparent` still darkens.
- **A posted closure is followed by `update_timers_and_animations`**
  (`i-slint-backend-winit-1.19.0/event_loop.rs:172-176`).
  A write from the winit filter still owes `request_redraw()` (`winit_filter.rs:397`), and
  `parked_loop.rs:110` stays because `arm_heartbeat` reads the next timer after it.
- **The winit backend reads the OS double-click time** (`lib.rs:1076-1095`) and nothing public
  exposes it, so the titlebar keeps its 500 ms.
- **Wayland copy goes through smithay-clipboard on the loop's own connection**
  (`clipboard.rs:139-156`), working on GNOME and in Flatpak.
- **The extractor** keeps its flags and PO writer, fails on a parse error, and on Windows writes
  `#:` locations with backslashes, `clean_path` rebuilding through `PathBuf::push`
  (`i-slint-compiler-1.19.0/source_path.rs:202-221`).
- **Window creation clears the launcher's activation token** through two `env::remove_var` calls
  (`winitwindowadapter.rs:727-737`), after the runtime's threads exist. Nothing for us to change.

### The dependency graph

muda 0.19 drops out (Slint shares tray-icon's 0.21); accesskit 0.25,
parley and fontique 0.12, taffy 0.15, femtovg 0.28 and resvg 0.49 move; Linux gains
smithay-clipboard with a second smithay-client-toolkit and loses the wl-clipboard-rs chain. Winit
stays `0.30.2`, so the `winit/` patch still applies, and zbus stays 5. Every new crate is MIT or
Apache-2.0/MIT.

### Not adopted

`ensure-visible` and `ensure-row-visible` scroll the least that shows a row, where
the queue seat, the dropdown reveal and the lyrics follow centre theirs; `backdrop-blur` blurs live
content, where our backdrops are pre-blurred covers.

## Phase 1: the bump

1. **Pin and lock**: `Cargo.toml`'s `slint` and `slint-build` to `1.19.0`, `cargo update -p slint
   -p slint-build`, the `Cargo.lock` diff read against the graph above, and
   `cargo install slint-tr-extractor --version 1.19.0 --locked`.
2. **Version stamps**: `CLAUDE.md`, `README.md`'s stack line, `licenses/ATTRIBUTION.txt`'s Slint
   entry, `.cargo/audit.toml`'s four Slint entries, the "Slint 1.18 has no" notes in
   `settings_page.rs`, `settings-page.slint` and `dialog-forms.slint`, and `slint.md`'s font
   registration line.
3. **Facts 1.19 falsifies**: `Cargo.toml`'s `renderer-femtovg` reason (slint#4176 is fixed),
   `drag_region.rs`'s interval docs, `parked_loop.rs`'s `//!`, `aurora-backdrop.slint`'s "never
   `transparent`" reason and `walk.rs`'s font-extension citation (`fileaccess.rs`).
   `tab-bar.slint`'s 0×0 note stays: a probe compiled on both versions folds `MaterialIcon` the same
   way. The hand test found open Hangul syllables composed whole on 1.19, so phase 2 retires
   `OPEN_HANGUL_EMS` rather than moving its stamp.
4. **One tile gradient**: the same probe showed a function's arguments reach gradient stops on
   1.18.1 and 1.19.0 alike, so the seven hand-spelled `@linear-gradient(135deg, …)` tiles go
   through `Theme.tile-gradient`, and `top_result_tests.rs` pins the new spelling.
5. **`update-translations.py`**: `#:` lines put back to forward slashes; a Linux run left every
   catalog unchanged.
6. **`slint-pitfalls.md`**: the moved anchors, the codegen tells, the `request_redraw` rule narrowed
   to filter writes, the rounded-clip softness clause, the drop-shadow cache key, the two
   measured-behind-`if` entries and the Hangul family order; the gradient-arguments entry cut.
7. **Gates**: fmt, clippy, the workspace tests, `cargo audit`, the build read for new Slint
   warnings, and the comment audit.
8. **Measure**: the generated `app-window.rs` grew from 60,228,208 to 64,420,471 bytes. Binary size
   and RSS wait for the last step.

## Phase 2: edition 1.19

1. **Opt in**: `build.rs` takes `.with_edition((1, 19))`, after checking today's build for
   binding-loop warnings, which the edition makes errors.
2. **Binding loops**: the two predicted are `NpEmptyState` mounts whose `beside` reads
   `root.height` (`lyrics-panel.slint`, `up-next-list.slint`), each pinned with `x`/`y` to keep
   today's geometry.
3. **Workarounds retired where the generated code agrees**: the `VerticalLayout` wrapper in the 23
   `views/settings/*-section.slint` files, tried on one first; each `preferred-width` pin on a
   layout cell, which 1.19 measures at the width it's given; `custom-titlebar.slint`'s 0×0 reason.
4. **`has-selection`**: `TextEditMenu` gates Cut and Copy on it, `has-text` keeping Select all.
5. **`OPEN_HANGUL_EMS` retired**: `measure.rs` charges every Hangul syllable the one em of the wide
   range, its test follows, and the loose-jamo pitfall goes.
6. **Docs**: the two edition-gated pitfalls and the edition in `slint.md`'s Project Layout.

## Phase 3: tests

On the go after the hand test.

## Last: measure

Binary size and idle and peak RSS from release builds of `main` and the branch, on the go.

## Hand test

Phase 1 passed in full on 2026-11-20.

1. **Go/no-go**: enter transitions, the queue sheet and nav transitions against 1.18.1.
2. Settings at narrow and wide widths, the onboarding features panel, hero and meta chips.
3. Typing in the tag editor, the smart-playlist editor and the settings search, and in a filter box
   after scrolling a composite view down.
4. Copy on Wayland (GNOME and KDE), the titlebar double-click, open Hangul lyrics, Persian elided
   labels, the aurora and hero backdrops through a theme fade.
5. Hide to tray and restore on X11, and idle CPU while hidden or minimized.
6. The name-hashed tiles, unchanged: Genres, a genre's detail hero and its search top result, a
   cover-less station card, its Radio banner and Now Playing.
7. Phase 2: the Now Playing and miniplayer empty states, the queue panel, Browse and Radio empty
   states, the tag editor, choice lists.
8. Windows, when next on it: the miniplayer swap during a modal move, and a translations run.

## Raised, not planned

- **The renderer premise**: 1.19's software renderer clips `border-radius`
  (`i-slint-renderer-software-1.19.0/lib.rs:3485-3510`), which ADR 0003's choice of FemtoVG rests
  on.
  A measured comparison is its own issue.
- **Slint's `SystemTrayIcon`** could replace our two tray backends and `tray-icon`, `tray_bridge`'s
  hide, raise and X11 activation staying ours.
