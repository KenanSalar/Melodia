# Crossfeed

Working doc. Delete when the feature ships.

Status: **Phases 1–5 done, Phase 6's hand tests done; Phase 5's two verify runs next, then Phase
6's tests** · Created: 2026-11-17 · Branch `feat/crossfeed`, linked to the issue

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

**Also fixed on this branch** (older than crossfeed, found by its hand tests, Phase 5):

- Once the safety limiter has caught a peak, the level comes back exactly to where it was, so the
  rest of the track is bit-identical again wherever the limiter rests.
- A track that ends on its own plays to its last sample, also where the next one reopens the output
  or the end of the queue gives an exclusive card back.

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
- The slider's readout names the nearest of the three: Subtle at the low end, Medium at
  `DEFAULT_STRENGTH`, Strong at the high end (`ui/crossfeed.rs`' `level`).
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

Both handlers call `library::crossfeed::switch_off(state, device)` on the UI thread before they
spawn anything; one already off is left unwritten. The reset alone (`ui/settings/signal_path.rs`)
names `player_heard_device`; the switch to exclusive (`output_settings.rs`'
`install_bit_perfect_switch`) names the new choice's `heard_device()`, the function the engine's
aim reads, so the device made bit-perfect is the one switched off. Switching it off after the
output switch, inside its task, would lose to the card: `player_set_output_choice` bumps
`output_changed`, and the card can paint the new device's crossfeed as on before the write lands.
It goes through `set_enabled`, so through `commit`, the map's one writer, rather than into
`reset_for_bit_perfect`'s write, which could land an older map after a newer.
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
  [graphic_eq] <the correction, or none set>       while it is on
               [Find Headphones] [Import] [Clear]
  [travel_explore] Find corrections online    [ ]  while it is on
  [swap_horiz] Crossfeed                       [ ]  disabled until the device is named
               Each ear hears a little of the other channel, as from speakers
  [tune]       Strength
               ======o=========  Medium        while it is on
```

- The device row moves out of the correction's stacked row to head the card, since both settings
  are per device; the correction's row takes the correction's own words as its label.
- Words per effect, as the catalogue has it: `CorrectionText` keeps the correction's (the
  onboarding row still reads its online pair), `CrossfeedText` holds the crossfeed's, and
  `HeadphonesText` the card's title and device line.
- The three icons are already in `scripts/icons.txt`.
- The readout's word is Rust's (`Crossfeed.level`), written back with the strength on every drag
  step, as RG's preamp is.

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
| facade | `crates/melodia-app/src/library/{headphones,crossfeed}.rs` | `DeviceTarget`, `Heard`, `heard()`; `crossfeed_for`, `set_enabled`, `preview_strength`, `set_strength`, `switch_off` |
| app glue | `library/playback.rs`, `state/mod.rs` | `player_set_crossfeed`; the shadow, seeded at startup; `hydrate_audio_dsp` hands the engine the map |
| card | `ui/views/settings/headphones-section.slint` (renamed from `correction-section.slint`), `components/settings/{headphones,crossfeed}-controls.slint`, `globals/audio.slint` | the device row first; `HeadphonesText`, `CrossfeedText`; a `Headphones` global (device, name, refresh) taken out of `HeadphoneCorrection`; a `Crossfeed` global |
| signal path | `ui/globals/signal-path.slint`, `ui/views/settings/output-section.slint` | `crossfeed-on`, four names |
| views | `crates/melodia-views/src/ui/headphones.rs`, `ui/crossfeed.rs`, `ui/correction/mod.rs`, `ui/settings/signal_path.rs` | the card's naming and repaint on `output_changed` move out of `correction`; crossfeed's switch and slider; the bit-perfect repaint |
| the two older bugs | `player/playback/limiter.rs`; the engine's `handlers.rs` and `backend/output.rs` | Phase 5; the limiter's smoothing in f64; the monitor waits out the device's lead at a natural end; no new module |

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
      `switch_off` (Phase 4 turned it from the heard device to a named one); `player_set_crossfeed`.
- [x] Lock order: only `CrossfeedShared::set`, an atomic store, runs under `HeadphoneControls`
      beyond what the correction already took.

**Gate:**
- [x] fmt, clippy, the suite; `signal_path_tests.rs`' `clean_inputs()` takes the field.

## Phase 4: The card and the signal path (`melodia-ui`, `melodia-views`)

- [x] The Headphones card: the device row first, the correction's rows, the Crossfeed switch and
      the Strength row; `has-matches` over every row; the page's comment counts no stages.
- [x] `ui/crossfeed.rs`: painted with the card's naming, the switch through `set_enabled`, the
      slider through `preview_strength` while dragged and `set_strength` on release.
- [x] Make Bit-Perfect: both handlers call `switch_off` first; `show_bit_perfect_reset` repaints.
- [x] The Effects row's fourth name.
- [x] Strings through `@tr`; `scripts/update-translations.py` and every catalog finished.
- [x] Docs: `README.md`'s features line, effects list and acknowledgments (bs2b's presets, Meier's
      circuit). `.claude/rules/ui-patterns.md` unchanged: its Settings line already covers a
      `*-controls.slint` and `*Text` per effect, and the card couples nothing across trees.

**Gate:**
- [x] fmt, clippy, the suite with the catalogs; the settings page pin on `category-name:` holds for
      the renamed section.

## Phase 5: Two older bugs the hand tests found

Both predate this branch and reproduce on `main`. They're fixed here on Kenan's call, each on its
own commit, before Phase 6's tests.

### 5a. The safety limiter never returns to unity

- [x] **What you hear:** nothing. After the limiter has caught one peak, the rest of the track
      plays 0.001 to 0.005 dB low, so a stretch it has left alone is no longer bit-identical to
      its source. That is why Phase 6's dual-mono hand test passes only "until the limiter first
      acts".
- **Where:** `Limiter::process` (`limiter.rs:42`) smooths `gain: f32` (`:15`) with
  `coeff.mul_add(self.gain, (1.0 - coeff) * target)` (`:45`), the release `coeff` from
  `smoothing_coeff` (`:66`) over `RELEASE_S`.
- **Why:** below 1.0 an f32 step is 2^-24. From `k` steps below, one release update lands `c·k`
  steps below (`c = exp(-1 / (RELEASE_S · rate))`), which rounds back to `k` once `k·(1 - c)` is
  under half a step, so the gain stops about `RELEASE_S · rate / 2` steps short.
  - Emulated with f32 rounding: 0.99986863 at 44.1 kHz (2204 steps, -0.00114 dB), -0.00124 dB at
    48 kHz, -0.00248 dB at 96, -0.00497 dB at 192.
  - The scratch harness running the real `EqSource` showed the same -0.0011 dB on every frame
    after the first catch.
  - It holds until `Limiter::reset` on a seek (`equalizer.rs:662`); each new source builds a
    fresh limiter at 1.0 (`equalizer.rs:343`).
  - Validated 2026-11-19: an exact-rounding emulation through the limiter's own f32 coefficient
    path stalls at 2205 steps at 44.1 kHz and 2400 at 48 kHz. One step from the 2204 above is one
    ulp of `exp` in the coefficient. The limiter runs at the source's rate (`equalizer.rs:307`),
    so 0.99986863 is the 44.1 kHz fixture's figure; a 48 kHz file stalls at 0.9998569.
- **Fix:** keep the smoothed gain in f64 and narrow it on return. Emulated, an f64 release narrows
  to exactly `1.0f32` 1.66 s after a -6 dB catch at 44.1, 48, 96 and 192 kHz.
  - `gain`, both coefficients, `ATTACK_S` and `RELEASE_S` in f64; `smoothing_coeff` in f64; `new`
    widens `frame_rate`. `target_gain` and the knee stay f32.
  - `process` keeps `f32` in and out, narrowing once a frame under its own
    `cast_possible_truncation` expectation.
  - `gain`'s doc keeps "at most 1.0" and gains why it is f64.
  - Not taken: snapping to the target within one step adds a threshold to argue; `1 - gain` in
    f32 decays through subnormals, where the f64 gain stalls about 2.4e-13 below 1.0, which
    narrows to exactly 1.0.
- **Verify:** rerun the dual-mono hand test (a 0 dBFS dual-mono file, crossfeed on against off:
  bit-identical except where the limiter acts and the release after it) and the loud-master one.

### 5b. A track's tail goes unplayed when it ends on its own and the output closes

- [x] **What you hear:** the end of a track cut off, up to the device's lead (80 ms at the default
      exclusive period, 400 ms at the longest), where it ends on its own and the next track
      reopens the output, or the end of the queue gives an exclusive card back. Heard only where
      the track doesn't end in silence.
- **Seen** 2026-11-19, exclusive on `snd-aloop` at 48 kHz S16, crossfeed off, the fallback set to
  shared. A 40 s mono FLAC followed by a 5.1 one the two-channel loopback refuses: the log reads
  "exclusive output refused ... the device cannot run 6 channels or more", then `Output
  reopened` on Default Audio Device. The mono file's last 912 frames read zero in the loopback
  capture; every frame before them is byte-identical.

  | queue | build | mono's last 912 frames |
  |---|---|---|
  | stereo (ends on its own) → mono → 5.1, twice | this branch | lost both times |
  | stereo (ends on its own) → mono → 5.1 | `main` | lost |
  | stereo (skipped 0.16 s in) → mono → 5.1 | this branch | played |
  | mono → 5.1 | this branch and `main` | played |

  - Always exactly 912 frames: one 960-frame period of the card (its 20 ms period; the buffer is
    3840 frames, 80 ms) less 48.
  - The stereo → mono reopen on the same card lost nothing of the stereo file, and the mono file
    started 14880 frames (310 ms) after it ended.
- **Why** (read off the tree 2026-11-19, confirmed by the runs below):
  - The voice runs dry a device's lead before the end is heard. The writer starts the card on a
    full buffer and refills it a period at a time (`alsa.rs:353-354`), so `finish_current`
    (`voice.rs:518`) drops the source count while the card still holds 60 to 80 ms of the track,
    with silence following it in (`mixer.rs:61`).
  - The monitor acts on the count alone. `evaluate_playback_check` reads zero as `EndOfStream`
    (`backend/mod.rs:58`), and `end_of_stream` (`handlers.rs:442`) either starts the next track
    through `cut_to_source` and `reopen_for_track` (`backend/mod.rs:320`,
    `backend/output.rs:429`) or stops, which `release_exclusive` follows (`backend/mod.rs:527`).
    Both drop the stream, and the writer drops the PCM undrained (`alsa.rs:480`).
  - The 500 ms poll doesn't cover the lead, because its ticks sit on a fixed grid
    (`handlers.rs:329`). The mono track started on the tick that caught the stereo's natural end,
    and its 40 s are 80 ticks, so the tick ending it lands after the voice runs dry and before the
    end is heard, by the same margin each run. Started by a skip or a click, off the grid, it
    played. Tracks of any length land at random: about lead / 500 ms of the natural ends that
    close the stream lose a tail, 16 % at the default period and 80 % at the longest.
  - The writer stops only once its blocking write returns at a period boundary
    (`alsa.rs:495-498`), so a cut comes in whole periods less the close: 912 is one period less
    48 frames, and a tick landing inside the last period cuts nothing.
  - In `x-off`, 05 starts 1,920,144 frames after the mono's cut, 80 ticks and 3 ms, so the grid
    holds across a run; 07 ended the queue whole, which fits a tick inside its last period.
- **Confirmed** 2026-11-19 on the rig above, release build of `2d1142fd`, queue 01 → mono → 5.1,
  the mono started by the tick that caught 01's end. Only the mono's length changes, by whole
  periods, so a block-grid fault would lose 912 frames in every row:

  | mono | lost | cut, frames into the mono |
  |---|---|---|
  | 40.000 s, the original | 912 | 1,919,088 (1999 periods + 48) |
  | 39.900 s, its first 1,915,200 frames | 0 | none |
  | 40.020 s, its last 960 frames repeated | 1824 | 1,919,136 (1999 periods + 96) |

  The cut stays on the same period boundary from the track's start, where the tick lands,
  whatever the track's length: a track ending before the tick plays whole, one ending later loses
  more. The close took 1 ms longer in the last row.
- **Fix:** the monitor waits out the device's lead before acting on a natural end.
  - `Monitor::end_of_stream` turns `async`: past the station check, `tokio::time::sleep` for
    `PlaybackEngine::output_lead()`, then it emits only if the status still reads Playing and
    `check_playback_state()` still reads `EndOfStream`, so a skip, stop or pause in the wait wins.
    No guard crosses the await. `Monitor::poll` turns `async` with it.
  - `output_lead()` in `backend/output.rs` reads `self.output.lead`, and `reads.rs`' clock read
    goes through it.
  - `end_of_stream`'s doc carries why it waits.
  - Skip, seek and stop never reach `end_of_stream` and still cut at once; gapless and crossfade
    are untouched. It covers every backend and the end of the queue at once, for up to one lead of
    extra gap at a natural end that isn't gapless or a crossfade.
  - Not taken: flushing the converter, which loses nothing; `pcm.drain()` at the close, ALSA's
    alone and needing a natural-end flag through `PlayMedia` and `stop`; waiting at any close with
    the voices dry, which also delays a user's stop on an exclusive card.
- **Reproduce:** `sudo modprobe snd-aloop`, then `arecord -D hw:CARD=Loopback,DEV=1 -f S16_LE -r
  48000 -c 2 -t raw` started before Melodia so it pins the card. Settings: `output_mode` exclusive,
  `output_device` `hw:CARD=Loopback,DEV=0`, `output_rate_fallback` shared, volume 100, and an
  `ALSA_CONFIG_PATH` whose default PCM is a null sink so the fallback plays nowhere audible. Queue
  three 40 s 48 kHz s16 FLACs cut from music that doesn't end in silence, titled so they sort
  stereo, mono, 5.1, and compare the capture with each file's `ffmpeg -f s16le` decode.
- **Verify:** the lossy queue at 40 s plays the mono file whole on this branch; the same queue
  ending on the mono file keeps its tail; a skip mid-track still cuts at once.

**Gate:**
- [x] fmt, clippy, the suite; `limiter.rs`' `gain` doc and `handlers.rs`' `end_of_stream` doc
      carry their fixes' why.

## Phase 6: Tests

Phases 1–5 gate on the static checks alone, each adding what it could break by hand to the list
below. The hand tests all run here, once everything above has landed; the tests follow, on
Kenan's go.

### Hand tests

In the dev data folder with a backup.

Measured 2026-11-19 on release builds of `2d1142fd` and `main`: 48 kHz fixtures (the graph's
clock), the shared stream pinned to F32 into a null sink and recorded from its monitor. Offline
figures come from a scratch harness running the app's decoder and `EqSource`, which matched every
in-app capture bit for bit.

- [x] The correction through Phase 1's moves: the card names the device heard and follows a
      switch; Find headphones, Import file and Paste land on the device shown; Clear, then Undo,
      restores it; a correction sounds as before, a device switch mid-track included.
      - Sounds as before: a ten-filter correction on two files, this branch against `main`,
        bit-identical over 90 s.
      - The card, driven exclusive between `snd-aloop` and the USB CODEC mid-track: it named each
        device on the switch; a file imported on the loopback, a paste and an AutoEq model on the
        CODEC each landed under that device's key in `eq.json` alone; Clear, then Ctrl+Z inside
        the notice, put the loopback's back.
- [x] Off: the bit-perfect rig and a flat chain read bit-identical, as before.
      - Flat chain: seven fixtures (stereo, dual mono, mono, 5.1) bit-identical to the decoder,
        gapless joins included.
      - Exclusive, S16 into `snd-aloop`: every two-channel fixture byte-identical to its file, and
        the mono one but for a tail `main` drops too (5b). The 5.1 one can't claim a
        two-channel card, so it falls back to shared.
- [x] On, Strong, an early stereo recording panned hard left and right: the image narrows, the
      level doesn't move; the four constants settled by ear and recorded here.
      - The dev library has no early stereo; its widest track (side above mid) and a one-ear track
        stand in. Widest: mid -0.001 dB (the limiter's stall, 5a), side -3.6 dB at Strong and
        -2.6 dB at Subtle, L/R correlation -0.13 to 0.28. One ear: correlation -0.01 to 0.54.
      - Kept by ear: `SUBTLE_FEED_DB` 9.5, `STRONG_FEED_DB` 4.5, `DEFAULT_STRENGTH` 0.7,
        `CUTOFF_HZ` 700.
- [x] A dual-mono stereo file with crossfeed on matches off sample for sample wherever the limiter
      rests.
      - Peaks at -6 dBFS: bit-identical on and off, in the app. Peaks at 0 dBFS: identical until
        the limiter first acts, 19 ms in, then never again: its release settles at 0.99986863
        (-0.0011 dB) in f32, not 1.0 (5a).
- [x] A mono file and a 5.1 file: untouched, the signal path reading as with it off.
      - Untouched: both bit-identical with crossfeed at Strong, the 5.1 file as its front pair.
      - The Signal Path block captured with crossfeed alone on and off: pixel-identical for both,
        Effects reading "No effects".
- [x] Toggling on and off and dragging the slider during a quiet passage: no click;
      `CROSSFEED_RAMP_MS` settled and recorded here.
      - Offline, switching every 2 s on a 100 Hz tone in one ear, the worst case for a click: an
        instant switch spikes 4.8 to 6.3 times the tone's own sample step, the 50 ms ramp 1.00 to
        1.09.
      - In the app, nine switches and slider drags on a one-ear song: none heard, and at each
        switch the capture's sharpest step within 30 ms matches the song's own. Kept at 50 ms.
- [x] Switching device mid-track: each keeps its own; System default on Linux shared.
      - At launch, exclusive on the loopback: on for System default only, the loopback plays
        byte-identical; on for the loopback, it matches the harness's render to -96.3 dBFS (the
        writer's dither) and the mono file stays byte-identical.
      - Mid-track: loopback at Subtle, the CODEC off (reading Bit-perfect), back to the loopback
        at Subtle, then shared with the loopback still picked reading System Default at Medium.
- [x] A loudly mastered hard-panned track at Strong: how often the limiter acts, against off.
      - FUNK UNIVERSO (Slowed), 149 s, peaks +2.8 dBFS. Off runs no limiter. Frames cut by more
        than 1 dB: limiter alone (an effect on, the signal unchanged) 37.7 %, mean -0.76 dB;
        Subtle 18.9 %, -0.58 dB; Strong 23.3 %, -0.60 dB. Crossfeed lowers hard-panned peaks, so
        it limits less than the limiter alone. The overshoot shows on centred loud material
        instead: Beirut Ya Beirut never passes 0 dBFS, crossfed at Strong 0.9 % of its frames do.
- [x] Integrated loudness of a few albums with it off and at each end, recorded here.
      - LUFS, ffmpeg's `ebur128` at 0.1 LU, whole albums offline. The drop doesn't follow the
        strength; the limiter alone accounts for 0.1 to 0.2 of it.

        | album | off | limiter alone | Subtle | default | Strong |
        |---|---|---|---|---|---|
        | The Harmony Band, Peace Be upon Us Album (12) | -10.4 | -10.6 | -10.9 | -10.9 | -10.9 |
        | Islamic Band, بحالي ربي أعلم (11) | -10.5 | -10.6 | -10.8 | -10.8 | -10.8 |
        | Islamic Band, الكعبة محلاها (8) | -11.3 | -11.4 | -11.7 | -11.7 | -11.7 |
- [x] Make Bit-Perfect switches off the heard device's crossfeed and keeps the others'; from shared
      output on Linux, with a search showing the Output and Headphones cards together, the card
      ends on the exclusive device with its switch off.
      - Searched "device", shared with the loopback picked: after the confirm, exclusive on the
        loopback, its crossfeed off at its kept strength, System default's still on; the card
        names the loopback with the switch off.
- [x] The readout changes word halfway between the default and each end (0.35 and 0.85 of the
      track at a default of 0.7), and the track holds still as it does, in English and in `nl`,
      whose "Gemiddeld" is the widest readout; `ru` and `el` for their scripts.
      - Released on each change of word: stored 0.367 and 0.858. Captured at 0.0, 0.7 and 1.0 in
        all four: every word fits, and the track ends at the same pixel in all twelve.
- [x] The Effects row names Crossfeed with a stereo file playing and not with a mono one, and
      lists four names in the order ReplayGain, correction, equalizer, crossfeed.
      - Captured, all four on: "ReplayGain, Headphone correction, Equalizer and Crossfeed" for a
        stereo file, "ReplayGain, Headphone correction and Equalizer" for a mono one.
- [x] The settings search finds the card by "headphone", "correction", "AutoEq" and "crossfeed".
      - Each typed and captured: the card shows for all four.
- [x] The cost on the audio thread with the correction and ten bands on, against without.
      - `cpal_alsa_out` at 48 kHz, six 20 s windows each: 4.81 ms/s off, 5.20 ms/s at Strong
        (0.48 % and 0.52 % of a core); 74.7 against 76.3 M instructions/s, about 33 a frame.

### Tests

Added where the landed change earns them, after the hand tests. Tests a phase breaks are fixed in
that phase.

---

## Cross-cutting

- **Docs land in the phase that stales them**, each argument here moving to the doc comment of what
  it constrains as it lands.
- **800 lines:** `equalizer.rs` (661, 679 after Phase 2) gains the scratch and the stage's hooks
  only; the stage is its own module. `backend/mod.rs` (677, 683 after Phase 3) renames a field.
  `library/playback.rs` (567, 573 after Phase 3) gains one setter. `output-section.slint` (453)
  gains a name and a chain. Phase 5's files have room: `limiter.rs` 68, `handlers.rs` 590,
  `backend/output.rs` 479.
- **Memory:** one f64 filter and a few scalars per source; nothing to measure past the hand tests.
- **Lock order** (ADR 0035) is unchanged: the renamed lock sits where the correction's did.
- **The live public release:** off by default; `crossfeed` is `#[serde(default)]`. `eq.json` has
  not shipped in a tag (`v0.23.0` predates `daa708df`), so `DeviceMap`'s rename costs no migration
  either way, and its shape stays.

## Open questions

- Whether the surround front-pair truncation gets its own issue (symptom only), and whether a
  real downmix later should run crossfeed after it.
