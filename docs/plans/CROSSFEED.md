# Crossfeed

Working doc. Delete when the feature ships.

Status: **Phases 1–3 done, Phase 4 waiting for its go** · Created: 2026-11-17 ·
Branch `feat/crossfeed`, linked to the issue

Issue [#157](https://github.com/KenanSalar/Melodia/issues/157).

> Verified **2026-11-17** against this tree at `644e1ce9`, bs2b's reference formulas (the `bs2b`
> crate's `src/lib.rs`, bs2b.sourceforge.net and its MIT licence page), Rockbox's
> `lib/rbcodec/dsp/crossfeed.c` (fetched through the GitHub API), `biquad` 0.6 (Context7
> `/korken89/biquad-rs`, the `Type` enum), Slint 1.18's `Slider` callbacks (Context7
> `/slint-ui/slint/v1.18.1`), and the rox and Strawberry checkouts. The figures below are those
> formulas evaluated at 44.1 kHz.

---

## The symptom

Settings ▸ Effects has three cards: Headphone correction, Equalizer and ReplayGain. None of them
changes how stereo is split between the ears. On headphones an instrument panned hard to one side
plays in one ear only, which is tiring over an album; on speakers both ears hear it. The Output
card's signal path names up to three effects.

## What ships

- The Headphone correction card becomes **Headphones**: the device the engine plays through on its
  first row, then the correction's rows as today, then a **Crossfeed** switch, off by default.
- While Crossfeed is on, a **Strength** slider runs from subtle to strong, reading Subtle, Medium
  or Strong beside it.
- Both are kept for each output device, System default counting as one: on for headphones, off for
  speakers. Switching device in Melodia switches them with it.
- Turning it on or off, moving the slider, or switching device ramps over a short time, so nothing
  clicks.
- Crossfeed leaves centre-panned sound (vocals, bass, a mono mix) exactly as it went in, so
  switching it on doesn't move the level. Only what differs between the channels is shared out. On
  a loud master the safety limiter, which runs whenever any effect does, still trims peaks near
  full scale.
- A mono or surround file passes through untouched (below).
- While it acts, the signal path's Effects row names Crossfeed and the path no longer reads
  Bit-perfect. Make Bit-Perfect switches it off for the device it makes bit-perfect.

**What keeps working:**

- With it off, playback is exactly as today, bit-perfect included.
- The equalizer, the preamp, ReplayGain, the correction and the safety limiter are untouched, and
  the limiter still runs last.
- Searching Settings for "headphone", "correction" or "AutoEq" still finds the card's rows.

---

## Validating the issue

Checked claim by claim against the tree:

- **Holds: the per-device memory exists.** #144 landed first, so this plan reuses it rather than
  building it. It is hard-wired to `Correction`, though (`melodia-core/src/entities/correction.rs`:
  `DeviceCorrections` at L141-150, its methods naming `Correction` at L153-174), so Phase 1 makes
  it generic.
- **Holds: Linux under System default sees no headphones.** `OutputChoice::heard_device()`
  (`backend/output.rs:53-60`) reads a shared stream as System default on Linux, so crossfeed is one
  setting there, as the correction is.
- **Holds: ahead of the limiter.** `EqSource::next_active` (`equalizer.rs:514-560`) runs the
  filters in f64, then the limiter on the frame's peak; crossfeed slots in between.
- **Holds: the signal path mechanism.** The Effects stage grades on `dsp_engaged`, which is
  `!self.bypass` (`equalizer.rs:629-631`), so a stage that clears the bypass reads Enhanced
  (`signal_path.rs:126`).
- **Wrong: "Surround files are mixed down for a stereo device after the effects run."** Nothing
  mixes them down. A shared stream opens at the device's own config (`output/device.rs:212-243`),
  and a source wider than a stereo device keeps its first two channels, the front pair, dropping
  centre, LFE and surrounds (`output/convert.rs:322-353`; only a mono device folds, by the mean).
  So the open question resolves to: **crossfeed acts on two-channel sources only.** Channels 0 and
  1 of a 5.1 file are half the mix, and crossfeeding them would act on a gap rather than on the
  music. The dropped channels are a separate problem worth an issue of their own.
- **Narrows the design: "doesn't make the music jump in level"** rules out bs2b, the usual
  choice (GStreamer's, FFmpeg's, Strawberry's). Its normalisation holds centre-panned bass at unity
  and lowers centre-panned sound above 1 kHz by 0.90–1.39 dB at 1 kHz and 1.06–1.75 dB at 16 kHz
  across its three presets. The complementary layout below holds it at exactly 0 dB.

---

## Decisions

### The algorithm: one low-pass on the difference

```
d  = L - R
c  = g · LP(d)        LP: first-order, 700 Hz, unity at DC
L' = L - c
R' = R + c
```

This is Jan Meier's passive circuit in its digital form: Rockbox's `crossfeed_meier_process` has
exactly this shape, its component values working out to `g = 0.2422` and a pole near 657 Hz.
Written per path it is a direct path `1 - g·LP` and a cross path `g·LP`, which sum to one, so:

- **Centre-panned sound is untouched**: `L = R` makes `d` zero, and a filter starting from rest
  on zeros outputs exact zeros, so crossfeed adds nothing to a mono mix in a stereo file.
- **Hard-panned sound reaches the far ear softened and late**: the cross path is low-passed, and
  the one-pole's group delay is 227 µs at low frequencies, near the ±30° speaker's interaural
  delay. That is what the issue asks for, with no delay line.
- **At DC it is bs2b**: the near and far ear take `1 - g` and `g`, which at the strong end match
  bs2b's default split (0.6267 / 0.3733) to four decimals. Above that bs2b loses level and this
  doesn't.

The filter is `biquad::Type::SinglePoleLowPass` through `dsp::design`, a `DirectForm1<f64>` as
every filter in the chain is. Its coefficients depend only on the cutoff and the source's rate, so
they are built once per source and never change.

### Strength is the amount `g`, and nothing else

- Strength is a 0–1 value. It maps to a feed level (how far below the near ear the far ear hears
  hard-panned bass) by straight interpolation from `SUBTLE_FEED_DB` 9.5 dB (Meier's) to
  `STRONG_FEED_DB` 4.5 dB (bs2b's ±30° speakers), and then `g = 1 / (1 + 10^(feed/20))`.
  `DEFAULT_STRENGTH` 0.7 lands on 6.0 dB, Chu Moy's level, which bs2b calls the most popular.
- The cutoff stays at `CUTOFF_HZ` 700 Hz at every strength.
- The slider's readout names the nearest of the three: Subtle, Medium, Strong.
- All four constants are starting points; the hand tests settle them by ear on hard-panned
  recordings and record the result here.

### Smooth changes: ramp `g`

The output is a straight line in `g` (`L' = L - g·LP(d)`, with `LP(d)` independent of `g`), so
ramping `g` linearly from its current value to a new target is exactly a linear crossfade between
the two settings' outputs. One mechanism covers on, off, a slider move and a device switch:

- A new target starts a ramp from wherever `g` sits, over `CROSSFEED_RAMP_MS` (50 ms to start, as
  `CORRECTION_SWAP_MS`), stepped once a frame. A target arriving mid-ramp restarts it from there,
  so a slider drag glides.
- No coefficient swap and no second bank: the filter runs on while `g` moves.
- Switching on starts the filter from rest; its settling is scaled by a `g` near zero.
- A new source takes the `g` set when it is built, with no ramp, so a gapless successor doesn't
  glide in from zero.
- A ramp landing on zero frees the bypass, so off is bit-identical again.
- A seek clears the filter and lands a ramp at its target, as the correction does
  (`correction/bank.rs` `reset`); digital silence clears it with the other filters in
  `settle_on_silence`.

### Where the stage sits

**In `EqSource`, on the whole frame after the bands, before narrowing to f32 and the limiter.**
The per-channel loop writes each channel's f64 output into a frame-sized f64 scratch, crossfeed
acts across it, and one loop narrows it and takes the peak. Order the samples meet:
ReplayGain/preamp, correction, bands, crossfeed, limiter, clamp, fade.

- Crossfeed needs both channels at once, which only the frame has. The correction and the bands
  treat both channels alike, so they commute with it and the order changes nothing audible.
- Steady-state, the two paths can add up to +0.58 dB (subtle) to +0.90 dB (strong) near 1 kHz on
  content phased to align, and a sudden change in the stereo image overshoots briefly while the
  cross path catches up. The limiter is the net, as the issue says; no headroom is computed.
- **Engaging it puts the source on the active path, limiter included**, as any effect does today:
  peaks inside the soft knee (from 3 dB below full scale, `limiter.rs`' `THRESHOLD_DB` and
  `KNEE_DB`) come down a little even where crossfeed itself changes nothing. A loud master is
  where switching on can still move the level, so the hand tests measure it.
- **Two channels only**: `crossfeed::reaches(channels)` is the one predicate, read by the stage
  and by the signal path. A mono or surround source never engages it.

### Publishing to the audio thread

`CrossfeedShared` holds the target `g` as one `AtomicF64`, zero meaning off. One cell needs no
sequence count or generation: the source compares it with its last target at each frame boundary.
It joins `DspControls` beside the correction. The engine hands it a `CrossfeedSetting` (`set`),
not `g`, so the strength-to-`g` mapping (`amount`) stays private to the DSP.

### One setting per device

- **`DeviceMap<T>`** in `melodia-core` (`entities/device_map.rs`): `system_default: Option<T>` and
  `devices: BTreeMap<String, T>`, with `for_device`, `insert` and `remove`, generic over what a
  device keeps. `DeviceCorrections` becomes `DeviceMap<Correction>` at every site, its on-disk
  shape unchanged (`eq_tests.rs` pins it).
- **`CrossfeedSetting { enabled, strength }`** in `entities/crossfeed.rs`, with `CrossfeedStrength`
  a 0–1 newtype through `TryFrom` (serde `try_from`), so a hand-edited value out of range resets
  the key rather than reaching the DSP. A device with no entry plays with crossfeed off and offers
  `DEFAULT_STRENGTH`. A device switched off keeps its strength.
- **The engine owns the heard device once.** `HeadphoneControls` (`backend/headphones.rs`) holds
  `heard`, `aim` and `heard_device`, and the two selects: `CorrectionControl` keeps its writer and
  map, its `select` now taking the heard device; `CrossfeedControl` publishes the heard device's
  setting (off where it has none) at every select, with no change test: a source ramps only to a
  target that moved. `aim` selects both. It is the engine's `correction` lock renamed, at the same
  place in the order.
- **Persisted in `settings.json`** as the `crossfeed` key: a switch and a strength are prefs, and
  `CLAUDE.md`'s state-file line already says so. A plain `SettingsData` field, the struct's own
  `#[serde(default)]` reading older files as empty; a bad strength resets only this key, the reader
  dropping one top-level key at a time.
- **One writer.** `AppState.crossfeed` is a `Shadowed<DeviceMap<CrossfeedSetting>>`, edited before
  every apply, each write changing one field in place (`DeviceMap::get_or_insert_default`) so none
  reads a setting back whole. `library::crossfeed`'s private `commit` hands the engine and the
  file the map as it stands, through `apply_latest`, the shape `library::correction::commit` has.
  `set_enabled` and `set_strength` commit; a slider drag goes through `preview_strength`, which
  edits the shadow and hands the engine the map at once, and its release through `set_strength`,
  as the Preamp's `set-preamp`/`commit-preamp` do.
- **A write's device comes from the card**, as the correction's does: `CorrectionTarget` and
  `heard()` move to `library::headphones` as `DeviceTarget` and `heard()`, shared by both.

### Make Bit-Perfect

Both handlers (`ui/settings/signal_path.rs:36`, `output_settings.rs:194`) end with
`library::crossfeed::switch_off_heard`, after the output switch where there is one, so the device
made bit-perfect is the one switched off. It goes through `set_enabled`, the map's one writer, rather
than into `reset_for_bit_perfect`'s write, which could land an older map after a newer.
`show_bit_perfect_reset` turns the card's switch off.

### The signal path

`SignalInputs.crossfeed_on`: the heard device's crossfeed on and the playing source two-channel.
`SignalPathUi.crossfeed-on` carries it, and the Effects row names four effects in the order the
samples meet them, gaining a four-name literal and a second-name chain for three of four.

### The card

```
Headphones
  [volume_up]  <device name>                       always, while the card shows
               Correction and crossfeed are kept for each device
  [headphones] Enable headphone correction    [ ]
     ... the correction's rows as today, while it is on ...
  [swap]       Crossfeed                       [ ]
               Each ear hears a little of the other channel, as from speakers
     Strength  ======o=========  Medium        while it is on
```

- The device row moves out of the correction's stacked row to head the card, since both settings
  are per device; the correction's row keeps its description and three buttons.
- Words per effect, as the catalogue has it: `CorrectionText` keeps the correction's (the
  onboarding row still reads its online pair), `CrossfeedText` holds the crossfeed's, and
  `HeadphonesText` the card's title and device line.
- The Crossfeed row takes `swap_horiz`, already in `scripts/icons.txt`.

---

## Prior art

- **Strawberry** (`src/engine/gstenginepipeline.cpp:899-905`, `:1017-1024`) links GStreamer's
  `bs2b` element when a checkbox is set (`src/settings/backendsettingspage.cpp:180`, `:440`).
  Not copied: bs2b's level drop, a single global switch, and a pipeline rebuild to toggle it.
- **rox** (`crates/rox-playback/src/convolver.rs:1056-1137`, `SpatialProcessor`): an integer
  delay line (`0.00026 · rate`, 12 samples at 48 kHz) and a one-pole on the far channel, scaled by
  `1 / (1 + 0.45 · 0.7)`. Not copied: that scale tilts centre-panned sound +0.85 dB at DC and
  -2.38 dB at the top at full strength, the delay quantises to the rate, and switching on jumps
  from the bypass with no ramp.
- **Rockbox** (`lib/rbcodec/dsp/crossfeed.c`, `crossfeed_meier_process`, GPL-2.0-or-later, read
  for the design only): the complementary shape this plan takes.
- **bs2b** (MIT): its three presets anchor the feed levels and the cutoff. Nothing of its code is
  used.

## What this does not do

- No crossfeed on mono or surround sources, and no downmix of surround: the converter keeping the
  front pair is its own issue.
- No delay line, no per-ear settings, no cutoff control, no virtual surround, room or impulse
  responses (the issue's scope).
- Not in the overflow menu's Equalizer dialog.
- An unplugged device with only a crossfeed setting is named "Not connected": the stored name the
  card falls back to is the correction's.

---

## Structure

| what | where | note |
|---|---|---|
| per-device map | `crates/melodia-core/src/entities/device_map.rs` | `DeviceMap<T>`, `get_or_insert_default` for a one-field edit; `correction.rs` loses `DeviceCorrections` |
| crossfeed types | `crates/melodia-core/src/entities/crossfeed.rs` | `CrossfeedStrength` (newtype, `TryFrom`), `CrossfeedSetting`, `DEFAULT_STRENGTH` |
| the stage | `crates/melodia-playback/src/player/playback/crossfeed.rs` | `CrossfeedShared`, `CrossfeedStage`, `reaches`, `amount`, the four constants. Beside `crossfade.rs` (the decks' ramp): one letter apart, so its `//!` says which it is |
| the ramp | `crates/melodia-playback/src/player/playback/dsp.rs` | `FrameRamp { start, target, frame, frames }` from the correction's private `Ramp` |
| the wrapper | `player/playback/equalizer.rs` | the f64 frame scratch, the stage polled with the others, bypass, seek, silence |
| engine | `backend/{headphones,correction,crossfeed,output,mod}.rs`, `signal_path.rs` | `HeadphoneControls` owns `heard` and `aim`; `CrossfeedControl`; `SignalInputs.crossfeed_on` |
| settings | `crates/melodia-app/src/services/settings/data.rs`, `library/settings/crossfeed.rs` | `SettingsData.crossfeed`; its setter |
| facade | `crates/melodia-app/src/library/{headphones,crossfeed}.rs` | `DeviceTarget`, `Heard`, `heard()`; `crossfeed_for`, `set_enabled`, `preview_strength`, `set_strength`, `switch_off_heard` |
| app glue | `library/playback.rs`, `state/mod.rs` | `player_set_crossfeed`; the shadow, seeded at startup; `hydrate_audio_dsp` hands the engine the map |
| card | `ui/views/settings/headphones-section.slint` (renamed from `correction-section.slint`), `components/settings/{headphones,crossfeed}-controls.slint`, `globals/audio.slint` | the device row first; `HeadphonesText`, `CrossfeedText`; a `Headphones` global (device, name, refresh) taken out of `HeadphoneCorrection`; a `Crossfeed` global |
| signal path | `ui/globals/signal-path.slint`, `ui/views/settings/output-section.slint` | `crossfeed-on`, four names |
| views | `crates/melodia-views/src/ui/headphones.rs`, `ui/crossfeed.rs`, `ui/correction/mod.rs`, `ui/settings/signal_path.rs` | the card's naming and repaint on `output_changed` move out of `correction`; crossfeed's switch and slider; the bit-perfect repaint |

`melodia-views` learns nothing about filters: it reads `CrossfeedSetting` from `melodia-core`
through `library::crossfeed`. The DSP learns nothing about devices: the engine hands it the heard
device's setting.

**The duplication this collapses first** (Phase 1):

- **The per-device map** becomes `DeviceMap<T>`, since a second kind of per-device value is what
  the issue asks for.
- **The heard device** moves out of `CorrectionControl` into `HeadphoneControls`, so one `aim`
  selects every per-device stage and `heard_device` has one home.
- **The card's naming** (`Card::refresh`, `heard`, `target`, the `asked` guard) moves out of
  `ui/correction/mod.rs` into `ui/headphones.rs`, and `CorrectionTarget`/`heard()` out of
  `library::correction` into `library::headphones`, so the crossfeed rows don't copy them.
- **The frame ramp**: the correction's `Ramp` becomes `dsp::FrameRamp`, which crossfeed shares.
  The decks' fade keeps its own counter: it counts interleaved samples and ends a deck mid-stream,
  semantics a frame ramp shouldn't carry.

---

## Phase 1: Groundwork (no audible change)

- [x] `DeviceMap<T>` in `melodia-core`, every `DeviceCorrections` site on `DeviceMap<Correction>`.
- [x] `HeadphoneControls` in the engine, owning `heard`, `aim` and `heard_device`;
      `CorrectionControl::select` taking the heard device; the lock renamed with its doc kept.
- [x] `library::headphones` with `DeviceTarget`, `Heard`, `heard()`; the correction facade and its
      views on them.
- [x] `ui/headphones.rs` holding the card's naming, `ui/correction` painting through it.
- [x] `dsp::FrameRamp`, the correction's bank on it.
- [x] Docs: the moved items' doc comments; `backend/mod.rs`' lock comment.

**Gate:**
- [x] fmt, clippy, the suite; the engine's `correction_tests.rs` follows the new select.

## Phase 2: The stage (`melodia-core`, `melodia-playback`)

- [x] `entities/crossfeed.rs`: `CrossfeedStrength`, `CrossfeedSetting`, `DEFAULT_STRENGTH`, serde.
- [x] `crossfeed.rs`: the constants, `amount`, `reaches`, `CrossfeedShared`, `CrossfeedStage`
      (the filter, `g`, the ramp, `poll`, `run` over a frame, `reset`, `reset_state`, `engaged`).
- [x] `EqSource`: the f64 frame scratch, the stage in `DspControls` and polled at the frame
      boundary, `rebuild`'s bypass counting it, seek and silence clearing it.
- [x] Docs: `playback/mod.rs`' chain order, `equalizer.rs`' `//!`. `unsafe-rust.md`'s "four
      iterator loops" line holds as written: still four, crossfeed adding one per-frame slice
      pattern. ADR 0010's shape (one wrapper, lock-free state) holds.

**Gate:**
- [x] fmt, clippy, the existing equalizer suite unchanged through `..DspControls::default()`.

## Phase 3: Per device, persisted (`melodia-engine`, `melodia-app`)

- [x] `CrossfeedControl` in `HeadphoneControls`, publishing at every select; `set_crossfeed`.
- [x] `SignalInputs.crossfeed_on`, fed in `signal_path()` from the control and `reaches`, under a
      narrow `struct_excessive_bools` expectation (one flag per effect the panel names).
- [x] `SettingsData.crossfeed`; `library::settings::set_crossfeed`.
- [x] `AppState.crossfeed`, seeded from the startup read; `hydrate_audio_dsp` hands it over before
      `set_output_choice` aims.
- [x] `library::crossfeed`: `crossfeed_for`, `set_enabled`, `preview_strength`, `set_strength`,
      `switch_off_heard`; `player_set_crossfeed`.
- [x] Lock order: only `CrossfeedShared::set`, an atomic store, runs under `HeadphoneControls`
      beyond what the correction already took.

**Gate:**
- [x] fmt, clippy, the suite; `signal_path_tests.rs`' `clean_inputs()` takes the field.

## Phase 4: The card and the signal path (`melodia-ui`, `melodia-views`)

- [ ] The Headphones card: the device row first, the correction's rows, the Crossfeed switch and
      the Strength row; `has-matches` over every row; the page's "three stages" comment made four.
- [ ] `ui/crossfeed.rs`: seed on the card's naming, the switch through `set_enabled`, the slider
      through `preview_strength` while dragged and `set_strength` on release, repaint on
      `output_changed`.
- [ ] Make Bit-Perfect: both handlers call `switch_off_heard`; `show_bit_perfect_reset` repaints.
- [ ] The Effects row's fourth name.
- [ ] Strings through `@tr`; `scripts/update-translations.py` and every catalog finished.
- [ ] Docs: `README.md`'s features line, effects list and acknowledgments (bs2b's presets, Meier's
      circuit); `.claude/rules/ui-patterns.md`'s Settings line if the card's shape earns one.

**Gate:**
- [ ] fmt, clippy, the suite with the catalogs; the settings page pin on `category-name:` holds for
      the renamed section.

## Phase 5: Tests

Phases 1–4 gate on the static checks alone, each adding what it could break by hand to the list
below. The hand tests all run here, once everything above has landed; the tests follow, on
Kenan's go.

### Hand tests

In the dev data folder with a backup.

- [ ] The correction through Phase 1's moves: the card names the device heard and follows a
      switch; Find headphones, Import file and Paste land on the device shown; Clear, then Undo,
      restores it; a correction sounds as before, a device switch mid-track included.
- [ ] Off: the bit-perfect rig and a flat chain read bit-identical, as before.
- [ ] On, Strong, an early stereo recording panned hard left and right: the image narrows, the
      level doesn't move; the four constants settled by ear and recorded here.
- [ ] A dual-mono stereo file with crossfeed on matches off sample for sample wherever the limiter
      rests.
- [ ] A mono file and a 5.1 file: untouched, the signal path reading as with it off.
- [ ] Toggling on and off and dragging the slider during a quiet passage: no click;
      `CROSSFEED_RAMP_MS` settled and recorded here.
- [ ] Switching device mid-track: each keeps its own; System default on Linux shared.
- [ ] A loudly mastered hard-panned track at Strong: how often the limiter acts, against off.
- [ ] Integrated loudness of a few albums with it off and at each end, recorded here.
- [ ] Make Bit-Perfect switches off the heard device's crossfeed and keeps the others'.
- [ ] The settings search finds the card by "headphone", "correction", "AutoEq" and "crossfeed".
- [ ] The cost on the audio thread with the correction and ten bands on, against without.

### Tests

Added where the landed change earns them, after the hand tests. Tests a phase breaks are fixed in
that phase.

---

## Cross-cutting

- **Docs land in the phase that stales them**, each argument here moving to the doc comment of what
  it constrains as it lands.
- **800 lines:** `equalizer.rs` (661, 679 after Phase 2) gains the scratch and the stage's hooks
  only; the stage is its own module. `backend/mod.rs` (677, 683 after Phase 3) renames a field.
  `library/playback.rs` (567, 573 after Phase 3) gains one setter. `output-section.slint` (453) gains a name and a
  chain.
- **Memory:** one f64 filter and a few scalars per source; nothing to measure past the hand tests.
- **Lock order** (ADR 0035) is unchanged: the renamed lock sits where the correction's did.
- **The live public release:** off by default; `crossfeed` is `#[serde(default)]`. `eq.json` has
  not shipped in a tag (`v0.23.0` predates `daa708df`), so `DeviceMap`'s rename costs no migration
  either way, and its shape stays.

## Open questions

- Whether the surround front-pair truncation gets its own issue (symptom only), and whether a
  real downmix later should run crossfeed after it.
