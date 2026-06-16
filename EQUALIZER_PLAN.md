# Plan: 10-Band Graphic Equalizer

## Context

Melodia plays audio through Rodio but offers no tone control — users can only set
volume and playback speed. The goal is a real **graphic equalizer** so users can
shape the sound to taste. The entry point is the now-playing **overflow (⋮) menu**,
mirroring how the playback-speed control lives there permanently. Clicking a new
**"Equalizer"** row opens the existing **reusable `Dialog`** as a modal panel with an
on/off toggle, a preset selector, ten vertical band sliders (±12 dB), and a Reset
button. Changes apply **live** to the currently-playing track (and the gapless-
preloaded next track) and **persist** across restarts.

### Decisions (confirmed with user)
- **Scope:** full graphic EQ (per-band sliders + presets + on/off), not presets-only.
- **Bands:** 10, classic ISO octave centres: 31, 62, 125, 250, 500, 1k, 2k, 4k, 8k, 16k Hz.
- **Surface:** the existing reusable `Dialog` (new `kind == "equalizer"` body branch).

## Why a custom DSP source (key technical finding)

Rodio 0.22.2 only ships `low_pass` / `high_pass` BLT filters (the source even comments
they're "probably buggy") — it has **no peaking/parametric filters**, so a real graphic
EQ cannot be built from rodio primitives. We add the **`biquad`** crate
(`korken89/biquad-rs`, no_std, well-regarded) which provides `Type::PeakingEQ(gain_db)`
+ `Coefficients::from_params`. We run each sample through a cascade of 10 peaking
biquads via a **custom Rodio `Source`** wrapper inserted right where the decoder is
built — so it automatically covers both the playing source and the gapless-preloaded
source (both go through the same decode path).

- Use **`DirectForm1`** (not `DirectForm2Transposed`): DF1's delay line stores past
  inputs/outputs that stay valid when coefficients change at runtime, so live slider
  drags don't inject transients. DF2T is for static filters only.
- Coefficients depend on **sample rate**, which differs per file (44.1k/48k). The shared
  state holds sample-rate-independent **params** (per-band gain in dB + enabled); each
  `EqSource` computes its own coefficients from its own `sample_rate()`.

## Architecture

```
UI (Slint)                       Rust callbacks            DSP / persistence
──────────                       ─────────────             ─────────────────
Overflow row "Equalizer"  ─────► opens Dialog(kind="equalizer")
EqualizerBody sliders/toggle ──► Equalizer.set-band(i, dB) ─► player_set_eq_band ─► EqShared (atomics, lock-free) ─► EqSource recomputes coeffs
                                                          └─► spawn_blocking ─► settings.equalizer (settings.json)
boot ◄── seed Equalizer global ◄─ state/mod.rs hydration ◄── read_settings + EqShared seed
```

EQ params live in **three** places, kept in sync: `EqShared` (DSP truth, read on the
audio thread), `settings.json` (persistence), and a new Slint `Equalizer` global (UI
binding). EQ is **orthogonal to the PlayerState machine** — unlike volume/speed it does
not need a `PlayerAction`/`with_state_emit` round-trip; it is applied directly on
`ctx.rodio` (which owns the `Arc<EqShared>`), exactly the level at which `set_volume` /
`set_speed` ultimately land.

## Implementation

### 1. Dependency — `Cargo.toml`
- `cargo search biquad` to pin the latest `x.y.z`, add `biquad = "x.y.z"` (per "deps:
  latest + hardcoded" convention).

### 2. DSP core — new `src/player/equalizer.rs` (+ `pub mod equalizer;` in `src/player/mod.rs`)
- Constants: `NUM_BANDS = 10`, `BAND_FREQS: [f32; 10]` (ISO centres above),
  `MIN_GAIN_DB = -12.0`, `MAX_GAIN_DB = 12.0`, `BAND_Q ≈ 1.4` (musical octave EQ),
  and built-in `PRESETS: &[(&str, [f32; 10])]` (Flat/Rock/Pop/Jazz/Classical/Bass
  Boost/Treble Boost/Vocal/Electronic). `Flat` = all zeros.
- `pub struct EqShared` — **lock-free** real-time state:
  - `enabled: AtomicBool`
  - `gains_bits: [AtomicU32; NUM_BANDS]` (f32 stored via `to_bits`/`from_bits`)
  - `generation: AtomicU64` (bumped on any change)
  - Methods: `set_enabled`, `set_gain(i, db)`, `set_all_gains(&[f32])`, each bumps
    `generation` (`Relaxed`); getters for hydration/UI seeding.
- `pub struct EqSource<S: Source>` wrapping the decoder:
  - Holds `Arc<EqShared>`, cached `last_generation`, `channels`, `sample_rate`,
    a channel cursor, and `filters: Vec<Vec<DirectForm1<f32>>>` (channels × bands).
  - `Iterator::next()`: pull inner sample; cheap `generation` compare; on mismatch
    recompute coefficients per band from the atomics + own `sample_rate` (preserving
    DF1 delay-line state via `update_coefficients`). **Bypass** (return raw sample) when
    `!enabled` or all gains ≈ 0 → **zero added cost when EQ is off** (the default).
    Otherwise run the sample through the current channel's band chain; advance cursor.
  - `Source` impl: forward `channels` / `sample_rate` / `current_span_len` /
    `total_duration` to inner; `try_seek` forwards to inner **and resets all filter
    state** (clear delay lines) to avoid a pop on seek.
- Memory: per-source state is a few hundred bytes (channels×bands DF1 structs), no
  caches — negligible, well within the RSS ceiling.

### 3. RodioPlayer integration — `src/player/rodio_backend.rs`
- Add `eq: Arc<EqShared>` field to `RodioPlayer`; construct in its `new`.
- Wrap the decoded source in **both** `play_media` and `preload_gapless`:
  `let source = decode_file(path)?; let source = EqSource::new(source, self.eq.clone()); player.append(source);`
  (`decode_file` stays a free fn; wrapping happens at the call sites that have `self`).
  Sharing one `Arc<EqShared>` across both means EQ stays consistent through gapless
  transitions automatically.
- Control methods (called from the library layer, lock-free, no track re-append):
  `set_eq_enabled(bool)`, `set_eq_band(usize, f32)`, `set_eq_gains(&[f32])`,
  plus getters `eq_enabled()` / `eq_gains()` for boot/UI seeding.

### 4. Persistence — `src/services/settings/data.rs` + new `src/library/settings/equalizer.rs`
- `EqualizerFlags` substruct (mirrors `PlaybackFlags`), `#[serde(default)]`, then
  `#[serde(flatten)] pub equalizer: EqualizerFlags` on `SettingsData`:
  ```rust
  pub struct EqualizerFlags {
      pub eq_enabled: bool,             // default false (visible behavior defaults off)
      pub eq_band_gains: Vec<f32>,      // default vec![0.0; NUM_BANDS]
      pub eq_selected_preset: String,   // default "Flat"
  }
  ```
  `#[serde(default)]` makes older `settings.json` files deserialize cleanly — no DB
  migration needed (settings.json is not the SQLite schema).
- Setters in `src/library/settings/equalizer.rs` (export from `settings/mod.rs`):
  `set_eq_enabled`, `set_eq_band_gains` (clamp each to ±12 dB, validate length),
  `set_eq_selected_preset` — each wraps `services::settings::mutate_settings`, matching
  `set_playback_speed`'s shape in `settings/playback.rs`.
- Player-apply fns in `src/library/playback.rs` (next to `player_set_playback_speed`):
  `player_set_eq_enabled`, `player_set_eq_band`, `player_set_eq_gains` — thin wrappers
  calling `ctx.rodio.set_eq_*` (no `with_state_emit`/`execute_actions` needed).

### 5. Boot hydration — `src/state/mod.rs` (`AppState::init`)
- Alongside the existing volume/speed block: read `settings.equalizer.*`, clamp gains,
  validate length (pad/truncate to `NUM_BANDS`), then seed `rodio`'s `EqShared`
  (`rodio.set_eq_gains(&gains); rodio.set_eq_enabled(enabled);`) **before** playback can
  start, so the first track is already EQ'd if enabled.

### 6. Slint UI
- **`ui/globals.slint` — new `Equalizer` global** (modeled on `Settings`/`Player`):
  - `in-out property <bool> enabled;`
  - `in-out property <[float]> band-gains;` (length 10, dB; backed by Rust `VecModel<f32>`)
  - `in-out property <int> preset-idx;`
  - `out property <float> min-gain; out property <float> max-gain;` (±12 for slider range)
  - callbacks: `set-enabled(bool)`, `set-band(int /*idx*/, float /*dB*/)`,
    `select-preset(int)`, `reset()`.
  - Band labels + preset names: per the i18n rule (`[string]` from Rust isn't
    translated), define them as **inline `@tr` literal lists in Slint**, ordered to
    match the Rust `PRESETS` / `BAND_FREQS` order.
- **New `ui/components/eq-band-slider.slint`** — a **vertical** band slider modeled on
  the volume-popup's vertical slider region (the horizontal `SliderTrack` header notes
  it is exactly that region rotated 90°). Maps a dB value in `[min,max]` to 0..1 thumb
  position; emits `changed(dB)` on drag and `committed(dB)` on release; shows a 0 dB
  centre reference. Reuse `SliderTrack`'s drag/commit/grab-zone logic.
- **New `ui/components/dialog/equalizer-body.slint`** — the dialog body:
  header row with a `ToggleSwitch` (`enabled`) + preset dropdown (inline `@tr` names,
  `preset-idx <=> Equalizer.preset-idx`), a horizontal strip of 10 `EqBandSlider`s with
  Hz labels under each, and a "Reset" `SectionButton`. Each slider's `changed` →
  `Equalizer.set-band(i, v)`; preset pick → `Equalizer.select-preset(idx)`; reset →
  `Equalizer.reset()`. Disable/dim sliders when `!Equalizer.enabled`.
- **`ui/components/dialog/dialog.slint`** — add one branch
  `if DialogGlobal.kind == "equalizer": EqualizerBody { }` (like the create-playlist /
  edit-playlist-artwork branches). Bump `card.max-w` for the `"equalizer"` kind so 10
  sliders have room. Set `confirm-label` = `@tr("Done")`, `cancel-label = ""` (no
  cancel; changes apply live). The generic `accepted`/`cancelled`/`closed` dispatchers
  in `globals.slint` need no EQ-specific routing (close-and-clear is correct).
- **`ui/components/now-playing/overflow-menu.slint`** — add an always-present
  `OverflowRow { icon: "graphic_eq"; label: @tr("Equalizer"); … }` (sibling of the
  Speed row). Its `clicked` closes the popup and opens the dialog:
  set `Dialog.kind = "equalizer"`, `Dialog.title = @tr("Equalizer")`,
  `Dialog.confirm-label = @tr("Done")`, `Dialog.cancel-label = ""`, `Dialog.open = true`.
  (First overflow row to open a Dialog — no existing precedent, so this is new wiring.)
  Add `graphic_eq` to `scripts/icons.txt` and re-run the subset script (per the Material
  Symbols subset rule); `scripts/check-icons.py` must pass.

### 7. Rust UI wiring — new `src/ui/equalizer.rs` (called from boot `ui_setup`)
- `install_equalizer(ui, state)`:
  - Seed the `Equalizer` global from `settings.equalizer` (enabled, `VecModel<f32>` of
    gains via `ModelRc::from`, preset-idx, min/max gain). Keep the `Rc<VecModel<f32>>`
    for later full-model replacement on preset/reset.
  - Wire callbacks with the established **two-phase** shape (see
    `src/ui/playback_settings.rs` / the `set_playback_speed` callback in
    `src/ui/callbacks/mod.rs`): **sync apply** to the live player first, then
    `spawn_blocking` persist.
    - `on_set_enabled(b)` → `player_set_eq_enabled` + persist `set_eq_enabled`.
    - `on_set_band(i, db)` → `player_set_eq_band` + persist updated `eq_band_gains`
      (also set `preset-idx` to the "Custom"/none sentinel since a manual edit leaves
      any named preset). Update the Slint model entry.
    - `on_select_preset(idx)` → look up `PRESETS[idx]`, push full gains to `EqShared`
      (`player_set_eq_gains`), replace the `VecModel<f32>`, persist gains + preset name.
    - `on_reset()` → apply Flat (all zeros), preset-idx → Flat, persist.

## Presets (built-in, Rust constants)
`Flat` (0…0), plus curated `Rock / Pop / Jazz / Classical / Bass Boost / Treble Boost /
Vocal / Electronic` as `[f32; 10]` dB arrays. "Custom" is a UI-only sentinel index shown
when the user hand-edits a band (not stored as a preset).

## i18n
Wrap every new visible literal in `@tr(...)` and add the msgids to **all** shipped
`translations/<lang>/LC_MESSAGES/Melodia.po`: "Equalizer", "Done", "Reset", "On"/"Off"
(if labeled), preset names, "Hz". Band-centre numbers (31, 62, …) stay numeric.
Preset/band label lists are **inline `@tr` literals in Slint** (order-matched to Rust),
per the "`[string]` from Rust renders whatever Rust pushed" workaround.

## Defaults & safety (live public release)
- EQ defaults **off**, all bands flat → zero audible change and **zero DSP cost** until a
  user opts in (bypass path in `EqSource::next`).
- New `settings.json` fields are `#[serde(default)]` → old installs upgrade safely; no
  SQLite migration involved.

## Files

**Create:** `src/player/equalizer.rs`, `src/library/settings/equalizer.rs`,
`src/ui/equalizer.rs`, `ui/components/dialog/equalizer-body.slint`,
`ui/components/eq-band-slider.slint`.

**Modify:** `Cargo.toml`, `src/player/mod.rs`, `src/player/rodio_backend.rs`,
`src/services/settings/data.rs`, `src/library/settings/mod.rs`,
`src/library/playback.rs`, `src/state/mod.rs`, `ui/globals.slint`,
`ui/components/dialog/dialog.slint`, `ui/components/now-playing/overflow-menu.slint`,
boot `ui_setup` (call `install_equalizer`), `scripts/icons.txt` (+ re-run subset),
and the seven `Melodia.po` files.

**Reuse (don't re-roll):** `SliderTrack` drag logic (`ui/components/slider-track.slint`),
`SectionButton`, `ToggleSwitch`, the `Dialog` overlay + `kind`-routing, the
`mutate_settings` / two-phase persist pattern, `PlaybackFlags` as the substruct template.

## Verification
1. `cargo clippy --all-targets -- -D warnings` (lint + typecheck; no `cargo check`).
2. `cargo test` — add unit tests in `src/player/tests/equalizer_tests.rs`
   (`#[cfg(test)] #[path = …]` per project convention): coefficient bypass when flat,
   gain mapping, length clamp/pad, seek resets filter state, and a settings
   round-trip (`EqualizerFlags` default + serde) test.
3. `cargo run` — play a track, open ⋮ → **Equalizer**: toggle on, drag bands (hear the
   change live), apply a preset, Reset, close. Restart → settings persist and re-apply.
   Verify a track change / gapless transition keeps the EQ applied. Verify EQ **off** is
   bit-identical to no-EQ (bypass).
4. `/usr/bin/time -v target/release/Melodia` once at the end — confirm peak RSS unchanged
   vs. baseline (EQ adds only a tiny per-source filter bank, no caches).
5. `python scripts/check-icons.py` passes after adding `graphic_eq`.

## Docs (post-implementation)
Update `CLAUDE.md` (new "Equalizer" convention paragraph: custom biquad `EqSource`,
`EqShared` lock-free atomics, Dialog `kind == "equalizer"`, `EqualizerFlags` persistence,
overflow entry). Commits are left to the user.

---

## Status: implemented ✅

All layers landed and verified: `cargo build` clean, `cargo clippy --all-targets -- -D
warnings` clean, `cargo test` green (738 incl. 10 new EQ DSP tests), all 6 `.po` files
`msgfmt -c` valid, `--version` + a 6 s startup smoke launch panic-free. `CLAUDE.md` updated.

Files created: `src/player/equalizer.rs` (+ `tests/equalizer_tests.rs`),
`src/library/settings/equalizer.rs`, `src/ui/equalizer.rs`,
`ui/components/dialog/equalizer-body.slint`, `ui/components/dialog/eq-band-slider.slint`.
Files modified: `Cargo.toml`, `src/player/{mod,rodio_backend}.rs`,
`src/services/settings/data.rs`, `src/library/{playback.rs, settings/mod.rs}`,
`src/state/mod.rs`, `src/boot/ui_setup.rs`, `src/ui/mod.rs`, `ui/globals.slint`,
`ui/app-window.slint`, `ui/components/dialog/dialog.slint`,
`ui/components/now-playing/overflow-menu.slint`, the 6 `Melodia.po` files.

Deviations from the plan as written:
- **`graphic_eq` icon was already in `scripts/icons.txt`** and subset into the fonts — no
  icon-subset re-run needed; `scripts/check-icons.py` step not required.
- **Globals need re-exporting in `app-window.slint`** — a Slint global is only reachable as
  `crate::Equalizer` from Rust if it's added to that file's `export { … }` block (and its
  import line), not merely `export`ed in `globals.slint`. Added there.
- **i18n**: `"Equalizer"` msgid already existed (play-button-animation label) and was reused;
  15 new strings (`On`/`Off`/`Done`/`Reset`/`Enable equalizer` + preset names) were added to
  all 6 `.po` files. English has no `.po` (source language).
- **Band slider + dialog body live under `ui/components/dialog/`** (next to `dialog.slint`),
  not `ui/components/`. The Reset button sits in the dialog body's control row; the footer
  keeps just "Done" (`cancel-label` empty).
- **`commit-band` is a distinct callback** from `set-band` (live vs. persist), mirroring
  `Player.set-volume` / `commit-volume`.

Not auto-verified (needs a human at the GUI): dragging sliders/hearing the change,
preset/reset behavior, persistence across a restart, and EQ surviving a gapless transition.
Run `cargo run`, play a track, open ⋮ → **Equalizer**.
