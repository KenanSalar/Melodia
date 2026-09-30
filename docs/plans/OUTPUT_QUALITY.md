# Output Quality

Working doc. Delete it when the last phase that's taken on ships.

Status: **Phases 1 and 2 complete** (tests held back, see Phase 2) · **Phase 3: Linux half
built, WASAPI half open** · Created: 2026-09-30 · Revised: 2026-09-30

> Facts below were checked on **2026-09-30** against `a0b9978b` on `feat/bit-perfect-output`,
> and against the pinned `cpal 0.18.2` sources. Line counts are from `wc -l` on that commit.
> Anything marked ⚠️ **re-verify** was not reachable without writing the code; check it on the day.

Every phase is sized to land on its own and be tried by ear before the next one starts. Only
Phases 1 → 2 → 3 depend on each other. Phases 4 to 8 can be taken in any order, or skipped.

---

## The symptom

When the output *can* be bit-perfect, it is. The problems all start when it can't be.

- **Music at a rate other than the device's is converted by linear interpolation.** In the default
  setup (Shared mode, "Match the File's Sample Rate" off), any track whose rate differs from the
  device's default config is converted this way. The most common case is 44.1 kHz music on a 48 kHz
  device:
  - On Windows this is every CD-rate track, because the mix format is usually 48 kHz and following
    the file's rate is not offered there.
  - On PipeWire it is whichever rate family the device's default config isn't in.

  Linear interpolation dulls the top octave and folds high-frequency images back into the audible
  band. Every playback speed other than 1.0 goes through the same interpolator, and at 2× it
  aliases.
- **An exclusive claim gives up when the device lacks the file's rate.** Examples: a 192 kHz file
  on a DAC that stops at 96 kHz, or 44.1 kHz on a 48 kHz-only output. The claim falls back to
  shared mode, the player bar's chip turns red, and a toast fires. An album that mixes rates flips
  the device between exclusive and shared track by track, with a gap at each flip.
- **Nothing is dithered.** When samples the chain changed (volume, EQ, ReplayGain, a crossfade,
  resampling, a lossy decode) are written to a 16-bit device, they are rounded. At low levels that
  rounding leaves correlated distortion rather than noise.

Smaller gaps, each a phase of its own below:
- **Crossfade is switched off for every track change under exclusive**, even between two tracks
  that need no reopen.
- **Shared output always goes to the system default device.**
- **Files are read and decoded on the exclusive writer's real-time thread.**
- **"Make Bit-Perfect" can't reach Bit-perfect in shared mode.**
- **A lossy track reads as Converted at the output stage.**
- **The WASAPI writer never counts an underrun.**
- **An exclusive claim is held through a pause of any length.**

## What ships, phase by phase

| # | Phase | Depends on | What the listener gets |
|---|---|---|---|
| 1 ✅ | The converter's state crosses a gapless seam | – | Nothing audible yet. It is what makes Phase 2 seamless. |
| 2 ✅ | A band-limited kernel in place of linear interpolation | 1 | Clean rate conversion and speed changes, in the default setup |
| 3 | Keep the claim on a rate the device lacks (a picker, default = today). **Linux half built, WASAPI half open** | 2 | A hi-res album on a capped DAC stays exclusive and gapless |
| 4 | TPDF dither where changed samples are narrowed | – | Noise instead of distortion on 16-bit output |
| 5 | Crossfade under exclusive when no reopen is needed | – | Crossfade works with exclusive between same-format tracks |
| 6 | A device picker for shared output | – | Music can go to a DAC without changing the system default |
| 7 | File reads on the writer thread: measure, then decide | – | Possibly nothing; a read-ahead only if the measurement shows stalls |
| 8 | Signal-path and claim polish (four small, independent items) | – | Honest words and parity fixes |

---

## Structure

Where each change lives, and how big each file is today. Production files stay under 800 lines.

| File | Lines | Phases | Role after the change |
|---|---|---|---|
| `crates/melodia-playback/src/player/playback/output/resample.rs` | new | 2 | Owns the kernel: the windowed-sinc table and the one function that evaluates a channel's window at a fractional position |
| `…/output/convert.rs` | 209 | 1, 2 | Keeps the stepping, the `Filled` accounting and the channel mapping. Its window is handed across a seam (Phase 1) and widened (Phase 2) |
| `…/output/voice.rs` | 628 | 1 | The handover passes the converter's state on when the successor's shape matches |
| `…/output/rates.rs` | new | 3 | The standard rate ladder and `device_rate_for`, the one rate policy both backends read |
| `…/output/alsa.rs` | 533 | 3 | Probes the ladder when the exact rate is refused |
| `…/output/wasapi.rs` | 665 | 3, 8c | Probes the ladder where `refusal` would say `RateRefused`; counts underruns |
| `…/output/mod.rs` | 703 | 3, 6 | `ExclusiveRequest` carries the rate policy; `OutputRequest::Shared` carries a device |
| `…/output/encode.rs` | 137 | 4 | `encode` takes a `Dither` |
| `…/output/device.rs` | 584 | 4, 6 | The shared `i16` arm takes the same `Dither`; the shared target by id |
| `crates/melodia-engine/src/player/engine/backend/mod.rs` | **784** | 5, 8d | Close to the cap: Phase 5 moves the refusal latch out before adding anything |
| `…/engine/backend/output.rs` | 350 | 5, 8d | Home of both reopen-refusal latches after Phase 5; a resume reclaims a parked claim |
| `…/engine/backend/controls.rs` | 106 | 5 | `crossfade_settings` stops blanket-disabling |
| `crates/melodia-playback/src/player/playback/crossfade.rs` | 384 | 5 | `crossfade_eligible` takes whether the next track plays without a reopen |
| `crates/melodia-engine/src/player/engine/signal_path.rs` | 142 | 8b | Names a lossy source |
| `crates/melodia-audio/src/player/source/audio.rs` | 171 | 8b | `SourceFormat` learns whether the codec was lossy |
| `crates/melodia-app/src/services/settings/playback.rs` | 258 | 3, 6, 8d | `OutputFlags` keys, all `#[serde(default)]` |
| `crates/melodia-app/src/library/playback.rs` | 541 | 8a | `player_make_bit_perfect` |
| `crates/melodia-views/src/ui/settings/output_settings.rs` | 270 | 3, 6, 8d | Output card wiring |
| `crates/melodia-ui/ui/views/settings/output-section.slint` | 348 | 3, 6, 8a, 8d | Output card rows |

---

## Phase 1 - The converter's state crosses a gapless seam ✅

**Why first:** Phase 2 widens the converter's window from two frames to a kernel's worth. Under
today's handover that would leave half a kernel of silence on each side of every gapless seam, and
it would happen in the default shared setup, where conversion is live. That is a tick on every
continuous album. Rebuilding the handover under the linear kernel first means the refactor is
validated by the existing suite before any sound changes.

**What changes**
- `Converter::fill` returns when its *source* runs dry while its window still holds frames that
  haven't been played yet. Today it holds the final frame and drains.
- `VoicePull::render` then looks at the staged successor.
  - **Same `Shape`:** the successor's converter takes over the outgoing one's window and
    fractional position. The buffers are the same size, so this is a swap with no allocation. The
    fill continues from the successor's source, and the unplayed tail comes out with the
    successor's first frames as lookahead.
  - **Different shape, or nothing staged:** the window drains with zeros past the end, as it
    effectively does today.
- Frames are counted against the source they were pulled from. So a handover mid-block starts the
  new clock at the right frame, and the handover lands a window's lookahead before the ear crosses
  it.
- A reset at a hard cut stays where it is: `start_at`, `replace` (a seek), `clear` and a shape
  change all start a fresh window.

**What stays the same**
- The converter is still built per source on the control thread.
- Nothing allocates on the audio thread.
- Step 1.0 is still an exact copy, including `-0.0`.
- `Mixer::reshape` still touches only the device shape.

**Done when**
- The static gates pass, with `tests/convert_tests.rs` and `tests/voice_tests.rs` adapted to the
  new `fill` contract.
- `crates/melodia/tests/{bit_perfect,stream_rate,crossfade}.rs` pass unchanged.
- A gapless album sounds exactly as it does today (manual check).

**Tests, after the manual go:** at a ratio other than 1, a sine split across two sources must
render exactly like the unsplit one.

**As built** (static gates green):
- The protocol is `Converter::{is_starved, drain}` plus `VoicePull::hand_over_or_drain`. The
  successor takes the converter over by a `mem::swap`.
- The window is paid straight after each write rather than before the next. That keeps a source
  whose last frame was just written from lingering a callback longer, which
  `voice_tests::a_staged_source_starts_when_the_one_ahead_of_it_ends` caught.
- A pre-existing fault was fixed on the way. Once a speed change had left the position part way
  between two frames, a return to exactly 1.0 kept interpolating while the verdict read
  Bit-perfect. A step of exactly one now drops that fraction.

## Phase 2 - A band-limited kernel ✅

**What changes**
- **New `output/resample.rs`.**
  - A windowed-sinc table, sampled at a fixed number of sub-positions between taps.
  - Built once per process in a `LazyLock` and shared read-only by every converter.
  - Forced by `AudioOutput`'s constructor on the control thread, so the audio thread never builds
    it.
  - One pure function evaluates a channel's window at a fractional position, with
    `stretch = step.max(1.0)` lowering the cutoff to the output's Nyquist whenever the output runs
    slower than the source (downsampling, or a speed above 1).
  - Start from a 128-tap Blackman-Harris kernel. Settle the length and window by measurement, not
    by argument.
- **`convert.rs`:**
  - The `prev`/`next` pair becomes a window of past and future frames per channel, sized for the
    capped stretch.
  - `write_frame` asks `resample` for each channel.
  - The linear `interpolate` helper goes.
- **Bit-exact bypass.** At a step of exactly 1.0, the output is still a straight copy of the
  window's centre frame. The past half starts as silence and the future half is lookahead, so frame
  0 comes out at output 0 with no leading gap, and `bit_perfect.rs` reads the same bytes.
- **Switching between bypass and kernel is seamless**, because both read the same window. A speed
  change or a reopen onto another rate never jumps.
- **Latency is folded into the lead.** The lookahead makes the pulled clock run half a window
  ahead of what is heard. That half-width goes into the lead `Voice::heard` subtracts, so published
  positions stay exact.
- **Cost is capped.** Taps per output sample grow with the stretch. The worst case is a high-rate
  source into a low-rate device at 2×. Cap the stretch at a value measured in release, and accept
  aliasing past it.
- **Docs.** Rewrite the stale module doc in `convert.rs`, which justifies linear interpolation.

**Memory:** one table per process. The per-source window grows from two frames to the kernel's
width. No new caches.

**Done when**
- The static gates pass.
- A release-build CPU check at the capped worst case, and an RSS read at idle and while playing.
- Kenan's ear check: a 44.1 kHz sine sweep in shared mode on a 48 kHz device, captured from the
  sink monitor, shows no images. A gapless live album has no ticks at the seams. A speed drag
  glides.

**Tests, after the manual go:**
- passthrough stays bit-exact at step 1;
- the stopband holds against a tone above the output's Nyquist;
- N input frames give `round(N·dst/src)` output frames once drained;
- the Phase 1 seam test, now at kernel width.

**Docs, after the manual go:** the chain line in `.claude/rules/audio-stack.md`, and the module
docs of `convert.rs` and `resample.rs`.

**As built** (static gates green). Where it differs from the plan above:

**Kernel**
- 128 taps, Blackman-Harris. The cutoff is placed half the window's main lobe below Nyquist.
- The table has 1024 rows per frame. It is laid out so a row holds every tap one side of the
  point sits at: an unstretched side reads two rows straight through, and takes its sum from
  precomputed row sums.
- A stretched side walks the table in 16-bit fixed point. `MAX_STRETCH` is 4.
- The table is forced by `Converter::new` rather than `AudioOutput`, since every converter is built
  on the control thread.

**Converter**
- Lookahead is pulled only while interpolating. At a step of exactly one the output copies the
  centre frame with no lookahead at all, so a bit-perfect path runs no later than the source, and
  the crossfade and bit-perfect suites see the same timing as before.
- A lookahead left behind when the step returns to one drains away as the centre moves through it.
- The end of a source drains against silence, and the converter is done when the centre reaches
  it.
- `Voice::heard` subtracts `converter_ahead`, published per render.

**Tests adapted**
- `voice_tests::the_position_counts_media_frames_rather_than_output_frames` reads `heard(ZERO)`.
  While interpolating, the pulled clock now runs the lookahead ahead of what was written.
- `crossfade.rs::a_staged_gapless_track_takes_over_through_an_output_reopen` allows the step
  between its two DC levels to ring by up to the Gibbs overshoot. The reopen puts that seam through
  the kernel, which rings on a hard step by design. A frame of silence would still dip almost to
  zero.

**Measured** in a throwaway release harness outside the repo, one stereo stream, on the Linux
machine.

CPU, as a share of one core:

| Conversion | Linear (before) | Kernel |
|---|---|---|
| Rates equal, speed 1 | 0.07 % | 0.06 % |
| 44.1 → 48 kHz | | 0.25 % |
| 48 → 44.1 kHz | | 0.66 % |
| 1.25× speed | | 0.72 % |
| 2× speed | | 1.09 % |
| 96 → 48 kHz | | 1.17 % |
| 192 → 48 kHz | | 2.30 % |

Error against the ideal sine, or leakage of a tone past the output's Nyquist:

| Case | Linear | Kernel |
|---|---|---|
| 44.1 → 48 kHz, 10 kHz tone, error | −15 dB | −130 dB |
| 44.1 → 48 kHz, 19 kHz tone, error | −4.9 dB | −123.5 dB |
| 48 → 44.1 kHz, 18 kHz tone, error | −7 dB | −117.5 dB |
| 96 → 48 kHz, 30 kHz tone, leakage | 0 dB | −140.7 dB |
| 2× speed, 15 kHz tone, leakage | 0 dB | −138.3 dB |

At integer ratios linear was near-exact in band (−157 dB at 96 → 48 kHz, 10 kHz), since it only
picks samples, but it aliased everything above. The kernel lands at −134 dB there.

**In-app run** (debug build, dev data folder backed up and restored). Shared output went to a
temporary null sink at 48 kHz, recorded from its monitor. Exclusive output went to the silent
PCM2902.

| Check | Result |
|---|---|
| 44.1 kHz 10 kHz tone → 48 kHz, image at 13.9 kHz | −166 dB; the only spur, −107 dB, is the 16-bit source's own |
| 19 kHz tone | full amplitude; image at 22.9 kHz −125 dB |
| 96 kHz file with 10 + 30 kHz | 30 kHz alias at 18 kHz −142.7 dB; 10 kHz level exact |
| 48 kHz noise at 48 kHz | bit-identical to the file over 3.68 s, both channels |
| Gapless pair split mid-waveform, through 44.1 → 48 kHz | one sine fits the whole run; no window above the source's −95 dB floor |
| 1.5× speed | 10 kHz plays at 15 kHz at full level |
| 1.5× speed, 19 kHz tone (28.5 kHz, past Nyquist) | steady leak −93 dB, the window's sidelobe floor |
| Exclusive | rate followed per track (44100, 48000, 44100 S16_LE); card released at the queue's end |
| Exclusive at 1.5× | claim held, no underruns logged |

In the scratch harness, eight live speed changes across one produced no step past a sine's
steepest slope, so nothing skipped or repeated.

**Listening pass** (Kenan, 2026-09-30, debug build). Each B track is the old linear converter's
output for the same decoded samples, rendered offline with the code from `HEAD` and played through
Melodia's copy path.

Shared, on the Elegiant speakers at 48 kHz:

| Test | Heard |
|---|---|
| Log sweep, new (A) against old (B) | A one clean tone; B a ghost tone rising beside it near the top |
| 96 kHz file with a 34 kHz tone, new against old | A only the quiet 1 kHz reference; B a 14 kHz whistle, measured at the output 18 dB above the reference |
| Gapless chord split mid-waveform | seamless |
| Speed drag across 1.0× | smooth |

Exclusive, on the Elegiant speakers (`hw:CARD=Generic_1,DEV=0`, ALC897):

| Test | Heard |
|---|---|
| Gapless chord | seamless, card held at 44100 S16_LE |
| 44.1 kHz chord then a 48 kHz one | a clean resync silence between them, card moved 44100 → 48000 |
| Speed drag across 1.0× | smooth |

No underruns were logged, and the card was handed back when Melodia quit.

**Still open**
- Tests: none written. They wait for Kenan's instruction or come from him. The candidates listed
  above still stand, plus a pin that a live speed change neither skips nor repeats a frame.
- Docs: the README Playback bullet and the two `.claude/rules/audio-stack.md` fixes are in.
- Release-build RSS.
- Found on the way, fixed separately: MPRIS reported `Rate` 1.0 at any speed, all three rate
  properties being hardcoded, so a client's progress bar ran at 1× until the next position update.
  - `Rate` now carries the live speed, and `MinimumRate`/`MaximumRate` carry the engine's bounds,
    closing on 1.0 while a station plays.
  - A change is announced with both bounds.
  - `Rate` is writable: 0.0 pauses, a negative or non-finite rate is refused, a set during a
    station is refused, and anything else is clamped, applied and persisted as the speed control
    does, through a new `PlayerEvent::SetSpeed`.
  - Checked live over D-Bus: 1.5 plays a 10 kHz tone at 15 kHz and persists; 3.0 clamps to 2; −1
    is refused; 0 pauses. The station case was not exercised live.

## Phase 3 - Keep the claim on a rate the device lacks (Linux half built, WASAPI half open)

**What changes**
- **New `output/rates.rs`:** the standard rate ladder, plus a pure
  `device_rate_for(source, supported) -> Option<u32>`. Preference order:
  1. The same rate family (44.1 kHz or 48 kHz multiples), at an integer multiple above the source.
  2. The highest rate in the same family below the source (192 → 96).
  3. The nearest rate in the other family (44.1 kHz on a 48 kHz-only output → 48).
- **ALSA (`alsa.rs`, `configure`):** where the rate read-back doesn't match, probe the ladder with
  `test_rate` on the same `HwParams`. That needs no reopen. Set the policy's pick and read it back.
- **WASAPI (`wasapi.rs`, `negotiate` / `refusal`):** where `refusal` would answer `RateRefused`,
  probe the ladder with `takes_exclusive` at each rate and run `candidates` at the pick.
- **Negotiated rate.** Either way the negotiated `Shape` carries the device's rate, not the
  source's. `Mixer::reshape` hands it to the voices, and Phase 2's kernel converts.
- **Setting: a picker, not a toggle, whose default is today's behaviour.** "When the device can't
  play the file's rate: Play through the system mixer / Resample and keep exclusive".
  - Persisted as a string key on `OutputFlags`, with `#[serde(default)]`.
  - It rides `ExclusiveRequest`, so changing it reopens the claim, the same way `hardware_volume`
    does.
  - It gets a row under Exclusive on the Output card.
  - Every new string goes into all six `.po` catalogues.

**What stays the same**
- The verdict: the rate stage already grades Converted when the device's rate isn't the source's.
  No Fallback, no toast.
- `FallbackReason::RateRefused` stays, for a device that offers no usable rate at all.
- `claim_serves` compares requests by the source's shape, so the second 192 kHz track on a capped
  DAC is served by the claim already open and plays gapless.

**Done when**
- The static gates pass.
- Kenan tries a file above the DAC's ceiling with the picker on Resample: it stays exclusive, the
  chip reads Converted at the device's rate, and there is no toast.
- With the picker on its default, behaviour is today's.

**Tests, after the manual go:** the `device_rate_for` table across both families and every edge,
and the new key's round-trip.

⚠️ **re-verify** that `wasapi`'s `is_supported` at a non-mix rate answers without an
`Initialize`, as the refusal classifier already assumes.

**As built, Linux half** (static gates green). Where it differs from the plan above, or pins what
the plan left open:

**Names**
- `RateFallback { Shared, Resample }` rides `ExclusiveRequest.rate_fallback` and
  `OutputChoice.rate_fallback`. It is persisted as `output_rate_fallback`, `"shared"` or
  `"resample"`, through `RateFallbackKey`, which mirrors `OutputModeKey`.
- The backend seam gains `RATE_FALLBACK`, surfaced as `RATE_FALLBACK_SUPPORTED`: true for ALSA,
  false for WASAPI and the unsupported backend. The picker row shows only where it is true.

**Policy (`rates.rs`)**
- `LADDER` runs from 8 kHz to 768 kHz. `device_rate_for(source: SampleRate, supported: &[u32])`.
- A rate's family is decided by divisibility: by 11 025 for the 44.1 kHz family, by 8 000 for the
  48 kHz one.
- Step 3 is "the nearest of the rest", ties going to the higher rate. That also covers a source in
  neither family.
- `mod rates` is declared in the Linux arm of `cfg_select!`, since nothing on Windows reads it yet.
  The WASAPI half adds it to its own arm, the way `hardware_volume` is listed in both.

**ALSA**
- `pick_rate` tests the source's rate first. Under Resample it then probes the ladder with
  `test_rate` on the same `HwParams`, sets the pick and reads it back.
- Only an exact-rate refusal takes that path. A driver that accepts a rate and then rounds it
  still refuses as `RateRefused`, as before.
- `Config` carries the device's rate, so `Negotiated.shape`, the reshape and the writer's lead all
  follow it, and `requested_period` is in device frames.

**UI**
- An "Unsupported Sample Rates" row sits under Exclusive Device, with the chips "Play Through the
  System Mixer" (the default) and "Resample and Keep Exclusive". The strings are in all six
  catalogues.

**Tests adapted:** `mod_tests::request` and the `alsa_tests` configure rows take `Shared`. The
settings round-trip carries `Resample`, since its contract is that every part of a choice
survives.

**In-app run** (debug build, dev data folder backed up and restored, on the silent UMC22,
`hw:CARD=CODEC,DEV=0`):

| File | Picker | Card (`hw_params`) |
|---|---|---|
| `04_s16_96000.wav` | Resample | 48000, S16_LE, period 960; `fallback: None` |
| `03_s16_88200.wav` | Resample | 44100, S16_LE, period 882; `fallback: None` |
| `04_s16_96000.wav` | Shared (default) | Refused as `RateRefused`, warned once, played shared at 96000 through the sound server (the UMC22 made the default output for the run) |

On quit through the tray the card was released, its own volume put back, and it returned to the
sound server under its original node name.

**Still open**
- The WASAPI half. That means the `negotiate`/`refusal` probe, flipping its `RATE_FALLBACK`,
  adding `mod rates` to the Windows arm, and sizing `requested_period` at the device's rate
  (it still reads the source's).
- The ⚠️ re-verify above.
- Tests: the `device_rate_for` table, the key's round-trip, and `RATE_FALLBACK_SUPPORTED` in the
  per-platform capability tests.
- Docs: `.claude/rules/audio-stack.md`'s list of seam names wants `RATE_FALLBACK`.
- Kenan's own check of the chip, which should read Converted with no toast.

## Phase 4 - TPDF dither where changed samples are narrowed

**What changes**
- **`encode.rs`:** `encode` takes a `&mut Dither`, a small per-stream PRNG with no new dependency.
  Each exclusive writer owns one next to its byte buffer.
- **Which rungs:** the S16 rung and the three 24-bit layouts. Never S32 or F32.
- **Which blocks:** only a block that doesn't already sit on the rung's grid. A bit-perfect block
  is exactly on the grid, so it passes untouched, and `bit_perfect.rs` and `encode_tests.rs` hold.
  Deciding per block, not per sample, is what keeps the error uncorrelated.
- **The shared path's `output_stream::<i16>`** (`device.rs`) takes the same `Dither`, for hosts
  that open a shared stream at 16 bits.
- **No setting.** It only touches samples that were already changed.

**Done when**
- The static gates pass, with the existing encode and bit-perfect tests unchanged.
- Kenan listens to a quiet fade on a 16-bit device with the volume below 100.

**Tests, after the manual go:**
- an on-grid block comes out bit-identical;
- an off-grid block's error stays within the dither's bound;
- the error of a low-level sine is uncorrelated with the sine.

## Phase 5 - Crossfade under exclusive when no reopen is needed

**The constraint that shapes it:** the crossfade builder can't refuse on its own.
`build_crossfade_actions` advances the queue when the fade *starts*. So if `begin_crossfade`
discovered the incoming track needs a reopen and backed out, the state machine would already be on
the next track while the old one played on. The decision has to be an input to the predicate that
decides the crossfade, not a failure after it.

**What changes**
- **`backend/mod.rs` is at 784 lines**, so first move `gapless_refused` out into
  `backend/output.rs`, beside `plays_without_reopen`. It becomes one latch type that both refusals
  share.
- **The monitor probes the next track's format once**, ahead of the crossfade window. It opens the
  file, reads the decoder's `shape()`/`format()`, and asks `plays_without_reopen`. The answer is
  latched per path and epoch, the way `gapless_refused` is today. The decoder stays the only source
  of truth for a format; the database's `bit_depth` column feeds nothing.
- **`crossfade_eligible` gains that answer as an input**, and `crossfade_settings` stops disabling
  crossfade wholesale while the output follows the rate.
- A track that needs a reopen gets exactly today's path: no fade, the gapless preload refuses it,
  and `EndOfStream → PlayMedia` reopens.
- **Manual crossfades** (a user skip) take the same answer. A refused one is the hard cut a skip
  already is.

**Done when**
- The static gates pass, with `crates/melodia/tests/crossfade.rs` unchanged.
- Kenan tries exclusive with crossfade on:
  - same-format tracks fade;
  - a rate change plays through to the end and reopens;
  - Settings no longer greys the crossfade rows out under exclusive.

**Tests, after the manual go:** the predicate's new input, and the latch's epoch handling.

## Phase 6 - A device picker for shared output

**Windows** is straightforward. cpal's WASAPI host lists endpoints with stable ids, and
`OutputRequest::Shared` gains `device: Option<String>`, which `default_target` becomes a lookup
for, falling back to the default when the id is gone.

**Linux is the open half, and the phase starts with a spike to decide it.** Through cpal's ALSA
host, a PCM named after a card bypasses the sound server, and would fight it for the card. What a
shared picker needs there is a *sound-server* sink. Candidates to read before choosing:
- cpal 0.18.2's own `pipewire` and `pulseaudio` hosts. Both are feature-gated. Both bring a C
  library to the build and to all five package formats, and the zbus footgun in the root
  `CLAUDE.md` has to be checked against the `pipewire` crate's features.
- Routing Melodia's one stream through the `pipewire-alsa` plugin's properties.
- The sibling checkouts' PipeWire output plugins, as prior art.

**Setting:** `OutputFlags::output_shared_device`, a stable id, or `None` for the system default.
The Output card's device row shows under Shared too, and the list is refreshed the way the
exclusive one is.

**Done when:** the spike's decision is recorded here, then the static gates pass, then Kenan picks
a non-default device on each platform.

## Phase 7 - File reads on the writer thread: measure, then decide

A file is decoded inline, on whichever thread pulls the mixer, which under exclusive is the
real-time writer. On a local SSD that's fine. On a network share, or a disk spinning up, a read
that blocks longer than the buffer leaves an audible gap.

- **Measure first:**
  1. Play from an SMB or NFS mount under exclusive at the 5 ms period.
  2. Read the ALSA writer's xrun count (`stream_health`'s counter) after a few albums.
  3. Compare against the same albums from a local disk.
- **Only if the share shows xruns:** a byte-level read-ahead for files. A feeder thread fills a
  bounded buffer under the `MediaSource`, and decoding stays inline. The stream path's ring and
  feed thread are the pattern.
- **If nothing shows:** close the phase with the measurement recorded here.

## Phase 8 - Signal-path and claim polish

Four independent items. Each lands on its own.

**8a. "Make Bit-Perfect" in shared mode.**
- Today the button turns off EQ, ReplayGain, speed and attenuation, and the verdict still reads
  Converted, because shared output can't be bit-perfect.
- Where `EXCLUSIVE_SUPPORTED`, the button also switches the output to Exclusive. Pressing a button
  labelled Bit-Perfect is the consent that needs.
- A refused claim already toasts, and the Fallback verdict explains itself.
- Code: `player_make_bit_perfect` (`library/playback.rs`) and `can-reset` (`output-section.slint`).

**8b. Name a lossy source.**
- Today an MP3 on an S32 claim grades Converted at the *output* stage. That is true, but it is the
  least interesting fact about the track.
- `SourceFormat` gains whether the codec was lossy, set in `decode.rs` from the codec type. A
  32-bit float WAV and an MP3 both decode to F32 today and can't be told apart.
- The voice publishes it with the other source atomics. `signal_path` names a lossy source in the
  verdict's words instead of pinning it on the output.
- New strings go into all six `.po` catalogues.

**8c. WASAPI underruns.**
- `Session::play` already measures `unplayed(written, played)` after each write.
- A clock that has caught up with everything written before a write means the device ran dry.
  Record that as `feed.health.record_xrun()`, the same counter ALSA's writer feeds.

**8d. Release the claim after a long pause.**
- Today a claim is held through a pause of any length.
- A setting, default off: "Give the device back after 5 minutes paused". The monitor parks the
  output past the threshold.
- `resume` then has to reclaim, which it doesn't today: `reopen_output` answers `None` for a parked
  output. The resume gains the reopen `reopen_for_track` does, for the source already on the deck.

---

## Not planned

- **macOS exclusive output:** stays issue #113. No macOS build ships and there is no Mac to
  validate on.
- **DSD (DoP and native) and ASIO:** no DSD decoder exists for Symphonia, so DSD would need a
  DSF/DFF demuxer, DoP framing in `encode`, DSD-to-PCM decimation for every other path, and tag
  reading. ASIO only earns its place alongside native DSD on Windows. Its SDK has been GPLv3 since
  October 2025, so licensing is no longer the blocker; demand is.
- **An integer path end to end:** `f32` carries every integer sample up to 24 bits exactly, and
  32-bit integer files are rare enough that grading them Converted is the honest answer.
- **Memory preload for sound quality, forced upsampling, a manual format override:** nothing to
  gain that a measurement would show. The format ladder is what the override would be for.
- **The three deferred items from the bit-perfect perf pass** (rtkit per writer start, hardware
  volume writes on the real-time thread, the busy-card retry at every track start): each starts
  from a measurement, not from this doc.

## Cross-cutting

- **Code, comments and this doc name no other player.**
- **The manual-test gate:** each phase stops at the static gates. New tests and doc updates wait
  for Kenan's go. Adapting an existing test to a changed signature is part of the phase, not new
  coverage.
- **Commits:** one commit per phase. Phase 1 and Phase 2 could be one commit if Phase 1's seam
  can't be validated on its own. It should be, through the existing suite.
- **Memory:** no phase adds a cache. Phase 2 adds one process-wide table and wider per-source
  windows; Phase 7 adds a bounded buffer only if measured.
- **Settings (Phases 3, 6, 8d):** new keys are `#[serde(default)]`, persisted as keys rather than
  indices, and ship off (today's behaviour).
- **Platform code:** the ALSA half is checked here and the WASAPI half on the Windows machine or by
  CI's `clippy-windows` and `test-windows`.

## Open questions

- Phase 2: the kernel length, window and stretch cap, all settled by measurement.
- Phase 3: whether Resample should be the default once it has shipped a release.
- Phase 6: the Linux mechanism, which the spike decides.

## Verification

After each phase:

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked --workspace -- -D warnings
cargo test --locked --workspace
```

- **Tests that stay green:** `crates/melodia/tests/bit_perfect.rs`, `stream_rate.rs` and
  `crossfade.rs`, both `…through_an_output_reopen` cases included.
- **Kenan runs the app** for the phase's own ear check. The app is never launched from here.
- **At the end of Phase 2:** a release build's CPU at the capped worst case, and `/usr/bin/time -v`
  peak RSS.
