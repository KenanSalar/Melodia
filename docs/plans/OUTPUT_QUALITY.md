# Output Quality

Working doc. Delete it when the last phase that's taken on ships.

Status: **Phases 1 to 5 complete**, Windows ear checks included (tests held back, see Phases 2 to
5) · **Phase 6: Linux half closed with no code, Windows half built and run** · **Phase 7:
dropped** · **Phase 8: all four built and run on Windows**, 8c's run having fixed event mode's
recovery from a stall · **Phase 9: 9B chosen, WASAPI half built and measured, its probe's cost
on a 7.1 output cut to one sweep per device and layout (built and run); ALSA half open
(Linux)** · Created: 2026-09-30 · Revised: 2026-10-02

> Facts below were checked on **2026-09-30** against `a0b9978b` on `feat/bit-perfect-output`,
> and against the pinned `cpal 0.18.2` sources. Line counts are from `wc -l` on that commit.
> Anything marked ⚠️ **re-verify** was not reachable without writing the code; check it on the day.

Every phase is sized to land on its own and be tried by ear before the next one starts. Only
Phases 1 → 2 → 3 depend on each other, and Phase 9 on 3. Phases 4 to 8 can be taken in any order,
or skipped.

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
| 3 ✅ | Keep the claim on a rate the device lacks (a picker, default = today) | 2 | A hi-res album on a capped DAC stays exclusive and gapless |
| 4 ✅ | TPDF dither where changed samples are narrowed | – | Noise instead of distortion on 16-bit output |
| 5 ✅ | Crossfade under exclusive when no reopen is needed | – | Crossfade works with exclusive between same-format tracks |
| 6 | A device picker for shared output. **Linux closed with no code (the sound server routes it), Windows built: one device for both modes** | – | Music can go to a DAC without changing the system default |
| 7 | File reads on the writer thread: measure, then decide. **Dropped (Kenan, 2026-10-01)** | – | Possibly nothing; a read-ahead only if the measurement shows stalls |
| 8 | Signal-path and claim polish (four small, independent items). **All four built** | – | Honest words and parity fixes |
| 9 | Gapless across a rate change the device doesn't see. **9B chosen; WASAPI half built, ALSA half open (Linux)** | 3 | A mixed-rate album stays gapless wherever the device's own rate doesn't change |

---

## Structure

Where each change lives, and how big each file is today. Production files stay under 800 lines.

| File | Lines | Phases | Role after the change |
|---|---|---|---|
| `crates/melodia-playback/src/player/playback/output/resample.rs` | new | 2 | Owns the kernel: the windowed-sinc table and the one function that evaluates a channel's window at a fractional position |
| `…/output/convert.rs` | 209 | 1, 2 | Keeps the stepping, the `Filled` accounting and the channel mapping. Its window is handed across a seam (Phase 1) and widened (Phase 2) |
| `…/output/voice.rs` | 628 | 1 | The handover passes the converter's state on when the successor's shape matches |
| `…/output/rates.rs` | new | 3, 9 | The standard rate ladder and `device_rate_for`, the one rate policy both backends read; 9B adds `RateSet` and `claim_rate` |
| `…/output/alsa.rs` | 533 | 3, 4, 9 | Probes the ladder when the exact rate is refused; dithers ahead of `encode`; 9B keeps the offered set (open, Linux) |
| `…/output/wasapi.rs` | 791 after 9B's cache | 3, 4, 6, 8c, 9 | Probes the ladder where `refusal` would say `RateRefused`; dithers ahead of `encode`; counts underruns and restarts an event-driven stream stuck after a stall; 9B probes the offered set before the first `Initialize`, once per device and layout, and picks a retry rate out of it; `SHARED_DEVICE` |
| `…/output/wasapi_offered.rs` | 48, new | 9 | The offered sets remembered for the process's life, keyed by what each sweep asked with |
| `…/output/wasapi_clock.rs` | 89, new | 8c | The device's clock read against the performance counter (`stood_still`, `StallWatch`), and WASAPI's 100 ns units |
| `…/output/mod.rs` | 781 after 9B's cache | 3, 6, 9 | `ExclusiveRequest` carries the rate policy; `OutputRequest::Shared` carries a device (Windows only), opened by `open_shared` |
| `…/output/claim.rs` | 144 after Phase 6 | 6, 9 | `claim_serves`, moved out of `mod.rs` to keep it under 800 lines, asks about the device's rate (9A) and the offered set (9B) |
| `…/output/dither.rs` | new | 4 | Owns the dither: which formats and blocks take it, the noise, and the quantize every writer calls before its conversion |
| `…/output/encode.rs` | 137 | 4 | Unchanged but for sharing `integer_bits` and `full_scale` with `dither` |
| `…/output/device.rs` | 626 after Phase 6 | 4, 6 | The shared 16- and 24-bit arms take the same `Dither`; `named_target`, the shared target by id |
| `crates/melodia-engine/src/player/engine/backend/mod.rs` | 786 after Phase 8 | 5 | The two position queries moved out to `reads.rs` |
| `…/engine/backend/reads.rs` | 29, new | 8d | The decks' read-only answers: the two positions, and whether the active deck holds a source |
| `…/engine/backend/output.rs` | 442 after Phase 6 | 5, 6, 8d | Home of the format latch both reopen refusals read; the pause-release setting's cell; `OutputChoice::shared_request` |
| `…/engine/backend/controls.rs` | 106 | 5 | `crossfade_settings` stops blanket-disabling |
| `…/engine/handlers.rs` | 595 after Phase 8 | 8d | `PauseWatch` and the release |
| `…/engine/state/transport.rs` | 456 after Phase 8 | 8d | `build_release_actions`, `build_replay_actions` |
| `crates/melodia-playback/src/player/playback/crossfade.rs` | 384 | 5 | `crossfade_eligible` takes whether the next track plays without a reopen |
| `crates/melodia-engine/src/player/engine/signal_path.rs` | 142 | 8b | Names a lossy source |
| `crates/melodia-audio/src/player/source/audio.rs` | 171 | 8b | `SourceFormat` learns whether the codec was lossy |
| `crates/melodia-app/src/services/settings/playback.rs` | 321 after Phase 6 | 3, 8d | `OutputFlags` keys, all `#[serde(default)]`. Phase 6 added none |
| `crates/melodia-app/src/library/playback.rs` | 567 after Phase 6 | 6, 8d | Play and toggle start a paused track its deck no longer holds; `SHARED_DEVICE_SUPPORTED` |
| `crates/melodia-views/src/ui/settings/output_settings.rs` | 366 after Phase 6 | 3, 6, 8d | Output card wiring |
| `crates/melodia-ui/ui/views/settings/output-section.slint` | 412 after Phase 6 | 3, 6, 8a, 8b, 8d | Output card rows |

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

## Phase 3 - Keep the claim on a rate the device lacks ✅

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
- The backend seam gains `RATE_FALLBACK`, surfaced as `RATE_FALLBACK_SUPPORTED`: true for ALSA
  and, since its own half, WASAPI; false for the unsupported backend. The picker row shows only
  where it is true.

**Policy (`rates.rs`)**
- `LADDER` runs from 8 kHz to 768 kHz. `device_rate_for(source: SampleRate, supported: &[u32])`.
- A rate's family is decided by divisibility: by 11 025 for the 44.1 kHz family, by 8 000 for the
  48 kHz one.
- Step 3 is "the nearest of the rest", ties going to the higher rate. That also covers a source in
  neither family.
- `mod rates` was declared in the Linux arm of `cfg_select!` first. The WASAPI half added it to
  the Windows arm, the way `hardware_volume` is listed in both.

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

**As built, WASAPI half** (static gates green on Windows, 2026-10-02). Only `wasapi.rs` changed,
plus `mod rates` in the Windows arm of `cfg_select!`. The setting, its row and its strings were
already platform-neutral, so there is nothing new to translate.

**Two attempts**
- `negotiate` first tries the source's rate through `open_first`, which is the old candidate loop.
- It tries again only when all three hold:
  - nothing took at the source's rate;
  - the request is Resample;
  - `refusal` answers `RateRefused`.

  The second attempt runs `open_first` at the rate `device_rate` picks. If that fails too, it
  tries the device's mix rate, the one the audio engine already runs the device at, since a
  driver can pass `IsFormatSupported` and still refuse `Initialize`. Only then does the claim
  return the first refusal. Where the pick is the mix rate, it is tried once.
- The gate is `RateRefused` because that verdict already means the device takes a candidate at
  its own mix rate. So a claim always has the mix rate to fall back on, even one off the ladder
  where `device_rate` finds nothing, and a refused channel count or format never pays for a
  ladder of `IsFormatSupported` calls.
- `device_rate` asks each `LADDER` rung except the source's own through `takes_at`, then hands
  what takes to `device_rate_for`. `takes_at` is the same question `refusal` now asks of the mix
  rate. The source's rate is left out because it has already been refused, and a driver can pass
  `IsFormatSupported` for it and still refuse `Initialize`.
- `RATE_FALLBACK` is true, so the picker row shows on Windows.
- `requested_period` is sized at the negotiated rate. `Session` already read every other size off
  the negotiated shape.

**The ⚠️ re-verify, settled statically.** `wasapi 0.24.0`'s `is_supported` under
`ShareMode::Exclusive` is a bare `IsFormatSupported` call. `negotiate` makes it on a probe client
that is never initialised, so no `Initialize` is involved at any rate. Whether a driver answers
honestly at a rate other than its mix rate is left to the in-app run.

**Found on the way, fixed beside it.** The Windows build was already red before this change, from
two leftovers of Phase 2's MPRIS `Rate` fix:
- `Published::rate_varies` is read only by the MPRIS backend, so it was dead code on Windows. It
  is now `#[cfg(target_os = "linux")]`.
- `souvlaki_backend_tests`' exhaustive match had no `PlayerEvent::SetSpeed` arm. It has one now.

**In-app run** (2026-10-02, debug build, Windows). The run used a scratch data folder through
`MELODIA_DATA_DIR`, seeded with the dev settings: exclusive output, polling, Hardware Volume on,
volume 25, crossfade off. Files were opened from the command line and the window was driven by
clicks. The fixtures were 16-bit stereo sines, 8 s each at −20 dBFS. Each claim was read off the
log's `Output reopened` line.

On the UMC22 (`Lautsprecher (2- USB Audio CODEC )`):

| File | Picker | Claim |
|---|---|---|
| 96 kHz | Resample | 48000 S16, `fallback: None`, `requested_period` 960; chip "Converted · 48 kHz", no toast |
| the same file repeated four times, then a second 96 kHz file | Resample | all gapless on that one claim, no reopen |
| 88.2 kHz | Resample | gapless staging refused for the format; reopened at 44100 S16, `requested_period` 882 |
| 48 kHz, then 44.1 kHz | Resample | 48000 and 44100, the files' own rates |
| 96 kHz, cold start | Shared (default) | refused as `RateRefused` and played shared on the system default; chip "Fallback · 96 kHz". No toast, since a cold start's toast is dropped before the bridge is up |
| 96 kHz, handed to the running window | Shared (default) | the same refusal, with the toast |
| the picker moved to Resample mid-track | | "Reopened for a new output choice" at 48000 S16 exclusive; the track carried on and the key persisted as `resample` |

- On that last claim the Signal Path panel read "96 kHz resampled to 48 kHz", and the Device row
  read "… 48 kHz · S16_LE · 2 ch, exclusive".
- The probe's cost:
  - the first claim, from a parked output, took 39 ms from play to the claim;
  - the reopen onto 88.2 kHz took 210 ms;
  - plain reopens at native rates took 162 and 186 ms.

On the ALC897 (`Elegiant speaker (Realtek(R) Audio)`), with Resample:

| File | Claim |
|---|---|
| 352.8 kHz | 44100 S16 |
| 176.4 kHz | 44100 S16 |
| 88.2 kHz | 44100 S16 |
| 192 kHz | 192000 S16, the file's own rate |

The Windows Realtek driver turns down 88.2 and 176.4 kHz even at the source's own rate, before
the ladder is asked. So the 44.1 kHz family stops at 44.1 kHz there, and the policy takes it over
the 96 and 192 kHz the device does have.

No warning or error was logged apart from the expected refusals, and every quit was clean.

**Listening pass** (Kenan, 2026-10-02). On the UMC22 at 48 kHz, converting from 96 kHz, one 440 Hz
sine was cut mid-waveform at sample 701 760 into two files. It played as one unbroken tone, with no
click, gap or crackle.

**Still open**
- Tests:
  - the `device_rate_for` table;
  - the key's round-trip.

  `RATE_FALLBACK_SUPPORTED` is pinned in `mod_tests`' two per-platform capability tests.
- Docs: done. The README's Unsupported Sample Rates bullet no longer says Linux only, and
  `.claude/rules/audio-stack.md` already names `RATE_FALLBACK` among the seam's names.

## Phase 4 - TPDF dither where changed samples are narrowed ✅

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

**As built** (static gates green). Where it differs from the plan above:

**Shape**
- The dither is its own step ahead of `encode`, in a new `output/dither.rs`, rather than a
  parameter of `encode`. So `encode` and `encode_tests.rs` are untouched, and the shared path
  makes the same call where cpal makes the integers rather than `encode`.
- `Dither::quantize(block, format)` runs between `fill` and the conversion in all three places a
  block is narrowed:
  - the ALSA writer;
  - the WASAPI writer, a blind cross-`cfg` edit left to CI's `clippy-windows` and `test-windows`;
  - the shared `output_stream`.
- The generator is a 64-bit LCG with Knuth's MMIX constants. Each sample takes one step, whose
  high 32 bits make two 16-bit uniform draws. Their difference is the TPDF.
- The math is in `f64`. A dithered sample is written back as an integer over a power of two, which
  `f32` holds exactly, so `encode`'s round and cpal's truncating cast both land on the integer meant.

**Which blocks**
- A block is left untouched when the format holds it exactly: every sample on a step of the grid
  *and short of full scale*. A sample at or past full scale counts as not held, so its block is
  dithered and clamped.
- S32 and F32 are never touched (`WIDEST_DITHERED_BITS` is 24).

**The shared path**
- `dithered_rung` maps `I16`/`U16` to the 16-bit grid and `I24`/`U24` to the 24-bit one. Every
  other format converts as before.
- **Found on the way, fixed by the same step.** cpal's `f32 → I24` conversion is unchecked, so a
  sample at or past full scale wrapped to the negative end, a click wherever one clipped. The EQ's
  clamp lands exactly on 1.0, and Phase 2's kernel overshoots on a loud master. Clamping the block
  first fixes it.
- cpal's `i16` conversion also truncates toward zero, which turned anything under one step into
  silence. A dithered block reaches it already on the grid.

**Measured** in a throwaway release harness outside the repo, driving `Dither::quantize` and
`encode` directly.

Passthrough:

| Block | Result |
|---|---|
| 16-bit noise into S16 and S24_3LE; 24-bit noise into S24_LE and S32_LE (24 valid), 2²⁰ samples each | bit-identical |
| 16-bit noise with one sample nudged off the grid, into S16 | the whole block dithered: 1059 of 4096 samples moved, the quarter TPDF takes off an integer |
| A changed block into S32 and F32 | untouched |

A 1 kHz sine into S16 at 48 kHz (dBFS; FFT of 65 536 samples, Blackman-Harris):

| Case | Path | Tone | Highest of harmonics 2 to 20 | Error's correlation with the signal |
|---|---|---|---|---|
| 16-bit source at −60 dBFS, volume 0.5 | input | −66.4 | −110.7 | |
| | rounded | −66.4 | −104.9 | +0.415 |
| | dithered | −66.4 | −110.6 | +0.002 |
| 16-bit source at −80 dBFS, volume 0.5 | input | −86.5 | −109.9 | |
| | rounded | −84.3 | −104.0 | +0.848 |
| | dithered | −86.5 | −110.4 | −0.001 |
| Float source at −85 dBFS (a lossy decode) | input | −85.4 | −235.1 | |
| | rounded | −85.0 | −101.6 | +0.172 |
| | dithered | −85.4 | −128.9 | −0.001 |

- In the 16-bit cases the harmonics at about −110 dBFS are the source's own 16-bit rounding,
  already in the input. Dithered output keeps exactly those and adds none.
- Rounding adds about 6 dB on top, and at −80 dBFS it moves the tone's level by 2.2 dB.
- For the float source, rounding leaves harmonics at −101.6 dBFS. Dithered, the highest bin near
  a harmonic is the noise, whose median bin sits at −137 dBFS.
- The error grows from 0.23 to 0.40 LSB rms rounded to 0.50 LSB rms dithered. Its largest value is
  1.48 LSB, inside the 1.5 bound, and its mean stays within 0.003 LSB of zero.

CPU at 96 kHz stereo, as a share of one core:

| Block | Per sample | Share |
|---|---|---|
| Changed, into S16, dithered | 2.00 ns | 0.038 % |
| Changed, into S24_LE, dithered | 1.35 ns | 0.026 % |
| Bit-perfect, into S16: the check alone | 1.38 ns | 0.027 % |
| `encode` alone into S16, for scale | 2.78 ns | 0.053 % |

A shared `I24` stream, the value the device reads:

| Sample | cpal alone | After `quantize` |
|---|---|---|
| 0.9999999 | 8 388 607 | 8 388 607 |
| 1.0 | −8 388 608 | 8 388 607 |
| 1.05 | −7 969 178 | 8 388 607 |
| −1.05 | 7 969 178 | −8 388 608 |

**In-app run** (debug build, dev data folder backed up and restored, Hardware Volume off, EQ,
ReplayGain and speed neutral). 48 kHz fixtures: 16- and 24-bit noise, and a 1 kHz sine at
−60 dBFS. The shared runs gave Melodia's process its own `ALSA_CONFIG_PATH`, which narrowed the
default PCM to one format and pointed it at a temporary null sink, recorded from its monitor as
float. The exclusive runs claimed `snd-aloop`'s card and recorded its capture side with `arecord`.

| Path | Case | Result |
|---|---|---|
| Shared `I16` | 16-bit noise, volume 100 | bit-identical to the file over all 240 000 frames |
| Shared `I16` | 16-bit sine, volume 50 | on the grid; error uncorrelated (+0.003, against −0.42 for the truncation it replaced); harmonics only the file's own (−110.1 dBFS); noise −136.9 dBFS per bin |
| Shared `I24` | full-scale 44.1 kHz square, resampled to 48 kHz | about 60 000 samples per channel past full scale, all held at the 24-bit limits, no sign flips |
| Shared `I24` | 16-bit noise, volume 100 | bit-identical, low 8 bits clear |
| Exclusive S16 (loopback) | 16-bit noise, volume 100 | bit-identical over all 240 000 frames |
| Exclusive S16 (loopback) | 16-bit sine, volume 50 | sample for sample the shared capture: 432 000 of 432 000 equal |
| Exclusive S24_3LE (loopback) | 24-bit noise, volume 100 | bit-identical over all 240 000 frames |
| Exclusive S24_3LE (loopback) | 24-bit sine, volume 50 | on the grid; error uncorrelated (−0.002, against ±0.76 rounded or truncated); harmonics only the file's own (−163.4 dBFS); noise −185.2 dBFS per bin |
| Exclusive S16 (PCM2902) | 16-bit sine, volume 50 | claim at 48 kHz, period 960, no fallback; no underruns at debug level; card released |

- The two dithered 16-bit captures are equal because the generator starts from its seed with every
  stream and draws nothing for silence or an on-grid block, so it meets a track's first sample in
  the same state on either path.
- The sound server reads the old `I24` containers as wrapped too. `aplay` of what cpal used to
  write for 1.0 and 1.05 came back from the null sink's monitor as −8 388 608 and −7 969 178.
- No underrun or warning was logged in any run.

**Listening pass** (Kenan, 2026-10-01, debug build). Exclusive on the Elegiant speakers
(`hw:CARD=Generic_1,DEV=0`, ALC897), held at 48000 S16_LE, Hardware Volume off:

| Test | Heard |
|---|---|
| A tone fading from −60 to −110 dBFS, quantized to 16 bits dithered (A) and rounded (B) by the harness, raised 40 dB after the quantize, played at volume 100 | as expected: A a clean fade into steady hiss, B rough and cutting to silence |
| A 16-bit 48 kHz track at volume 15, with the slider moved across 100 and back, pauses, resumes and seeks | as expected: no clicks or crackles, the dither switching on and off unheard |

No underrun was logged, and the card was handed back when Melodia quit.

**Listening pass, Windows** (Kenan, 2026-10-02). The UMC22 ran exclusive at 48000 S16, Hardware
Volume off so the voices scaled the samples and the WASAPI writer dithered them. The arpeggio fixture
looped at volume 15. Kenan moved the slider to 100 and back, paused and resumed three times, and
sought twice. There were no clicks or crackles, and none of it logged an underrun or a warning.

**Still open**
- Tests: none written. The candidates above stand, plus: S32 and F32 come out untouched; a shared
  `I24` sample past full scale holds at the maximum; `bit_perfect.rs` runs `quantize` the way the
  writers do.
- Docs: done. The chain line and a bullet in `.claude/rules/audio-stack.md` (every narrowing
  site calls `Dither::quantize` first), and a README bullet under Bit-perfect output.

## Phase 5 - Crossfade under exclusive when no reopen is needed ✅

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

**As built** (static gates green). Where it differs from the plan above:

**The latch**
- `gapless_refused` is gone. `EngineOutput.probed` holds the last file a track-boundary decision
  opened and what it found: `Decodes(Shape, SourceFormat)`, or `Unreadable`.
- It keeps the format, not the verdict. `plays_without_reopen` is asked again each time, of the
  output as it stands, so there is no epoch key and an output-choice change can't leave a stale
  answer behind.
- `reopens_for(path)` is the monitor's way in. `preload_gapless` reads the same record through
  `recorded_answer` and opens through `open_recorded`, so a refused or unreadable next track is
  opened once rather than on every tick.

**The decision**
- Before its snapshot, and ahead of the positions in it, the monitor asks `reopens_for` about the
  track queued next. It asks only while crossfade is on, and not while a station plays. It asks
  whether the output follows the rate or not: turning that off leaves the stream open at the
  followed rate until the next cut, and `start_track` would cut a crossfade the monitor had let
  through. `BackendSnapshot.next_needs_reopen` carries the answer.
- The answer is folded into `crossfade_eligible`'s existing term rather than added as a fifth.
  `pause_at_end` became `drains_to_end`: both mean the track has to reach `EndOfStream`. The
  predicate's tests are unchanged.
- `crossfade_settings` passes the user's settings through.

**Where it commits**
- `play_media` and `begin_crossfade` share `start_track`, which takes an `Entry::{Fade, Cut}`.
  Under the decks lock it turns a fade into a cut from the top when a gapless source is staged or
  the output has to reopen. So a manual skip into such a track is a hard cut, and an automatic
  crossfade whose decision went stale becomes one too.
- A fade never reopens. A shared follow-rate stream's same-rate reclaim, and a fallen-back
  claim's retry, wait for the next cut, as they already do across a gapless transition.
- `backend/mod.rs` ends at 798 lines. The latch moved out, but the shared helper took most of
  that back.

**UI**
- The Crossfade rows stay live while the output follows the rate. The toggle's description there
  reads "Overlap the end of one track with the start of the next, unless the device has to change
  format between them". "Match the File's Sample Rate" no longer says it turns crossfade off.
  Both strings are in all six catalogues, and the two they replace are removed.

**Tests adapted:** the three `backend_tests` that pinned crossfade off under following the rate
are deleted, and `handlers_tests::backend` sets `next_needs_reopen: false`.

**In-app run** (debug build, dev data folder backed up and restored, crossfade 2 s with "Except
on the same album" off, volume 100). The fixtures were 16 s sines, a different tone per track:
01 and 02 at 48 kHz, 03 and 04 at 44.1 kHz, 05 at 44.1 kHz 24-bit, and 06 at 48 kHz. Shared
output went to a temporary null sink, recorded from its monitor. Exclusive output went to the
silent PCM2902, watched through its `hw_params`, and to `snd-aloop`, recorded from its capture
side. A crossfade shows as an overlap of the two tones whose levels sum to 0.500, the level of
either tone alone.

| Run | 01→02 | 02→03 | 03→04 | 04→05 |
|---|---|---|---|---|
| Shared, rate not followed (the baseline) | fade | fade | fade | fade |
| Shared, rate followed | fade | no fade; 0.21 s silence; reopened at 44100 | fade | fade |
| Shared, rate followed, manual skips | fade, no reopen | hard cut; 0.21 s silence; reopened at 44100 | fade, no reopen | |
| Exclusive (PCM2902) | fade; card held at S16_LE 48000 | no fade; card reopened at 44100 | fade; card held at 44100 | no fade; the 24-bit claim was refused and the track played shared, as before |
| Exclusive (PCM2902), manual skips | card held | card reopened at 44100 | card held | |

- On `snd-aloop` (all three tracks at 48 kHz), the automatic 01→02 fade and a manual skip 02→06
  both overlapped with levels summing to 0.500, and the card stayed at S16_LE 48000.
- Outside the fades the capture was bit-identical to the source: 14 s of 01 before its fade, and
  13 s of 06 between its fade-in and its end. Inside a fade the channels differ by at most 2 LSB,
  which is the dither working on changed samples.
- In every fade the two levels summed to exactly 0.500, so no step clipped or dipped.
- Re-run after the probe stopped gating on whether the output follows the rate:
  - The default Shared baseline still faded at all four hand-overs, with no reopen.
  - With rate-matching switched off mid-track (Kenan's click), the stream was still open at
    44100. The track ended at `EndOfStream`, logged as `play` rather than `crossfade`, and the
    output reopened at the default 48000 with 0.49 s of silence. The next transition faded.
    Before the change, that hand-over decided a crossfade that `start_track` then cut.

**Settings check** (Kenan, 2026-10-01): under Exclusive, the Crossfade toggle is live with the
format note, and turning it on shows the four detail rows.

**Still open**
- Tests. The epoch candidate above no longer applies. In its place:
  - an `evaluate_playing_tick` row where `next_needs_reopen` blocks the crossfade and lets the
    preload through;
  - `crossfade_settings` passing the user's choice through under following the rate;
  - the latch answering a second ask after the file is deleted, which shows it opened once.
- Docs: done. `.claude/rules/audio-stack.md`'s "A format change is decided at the boundary"
  bullet is rewritten around `plays_without_reopen` and the record both readers share, its gapless
  note names `start_track`, and the README crossfade bullet covers exclusive output.

**Windows run and listening pass** (2026-10-02). The UMC22 ran exclusive, crossfade 3 s, through a
queue of three 12 s tones: 440 Hz and 660 Hz at 48 kHz, then 550 Hz at 44.1 kHz.
- The log read `crossfade 2720ms` into the second tone, the time the first had left, with no reopen.
- The third tone came by `play`, with the claim reopened at 44100.
- Kenan heard the first pair blend smoothly, and the second play through with a short gap and no
  click.
- Settings under Exclusive showed the Crossfade toggle live with the format note, and its four rows.

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

**Spike, Linux (2026-10-01): no in-app picker. The sound server is the picker.**

What already works:
- `main.rs` sets `PIPEWIRE_ALSA` so Melodia's stream carries `application.name` and `node.name`
  "Melodia" on every open.
- WirePlumber 0.5 remembers the sink a user moves a stream to, keyed on that name
  (`node.stream.restore-target`, on by default). It applies that sink to every stream Melodia opens
  afterwards, so a reopen at a new rate and a relaunch both land back on it.
- PulseAudio's `module-stream-restore` does the same with the ALSA plugin's client name.
- Plasma's audio applet and pavucontrol both move a stream.

Every in-app route costs more than it gives:

| Route | Why not |
|---|---|
| cpal's `pulseaudio` host (pure Rust) | The callback runs on the protocol client's one network thread, holding its state lock, never real-time. Dropping a stream the server killed can hang inside a reopen that holds the output mutex. It prefills about 2 s of server buffer, and drops underflow, move and kill events. The client is named `cpal-pulseaudio-<pid>`, which renames Melodia in every mixer and defeats the routing memory above. The feature also changes what `cpal::default_host()` returns. |
| cpal's `pipewire` host | libclang, the PipeWire headers and pkg-config at build time, in CI and for all five package formats. A hard link to `libpipewire-0.3`, so the binary won't start without PipeWire (Ubuntu 22.04 ships PulseAudio). Its device list is a snapshot taken at host creation, and includes other apps' streams. |
| Our own `alsa` open of `pipewire:NODE=<name>`, with a pure-Rust pulse client to list sinks | A second shared writer beside cpal's. Works only where pipewire-alsa is installed, and PulseAudio installs no equivalent PCM by default. A new dependency just to list the sinks. |
| Moving our own stream over the pulse protocol after cpal opens it | Races WirePlumber's own memory of the stream's target, and the crate wraps no move command. |

So the shared-device setting and its row are Windows-only, built with the Windows half behind a
capability constant, the way `FOLLOW_RATE_SUPPORTED` leaves Windows out.

**Checked in-app** (debug build, dev data folder backed up and restored, shared output, Match the
File's Sample Rate on, volume 0):

| Step | Melodia's stream |
|---|---|
| Launch | on the default sink |
| `pactl move-sink-input` to a temporary null sink, as Plasma's applet does | moved; WirePlumber stored `"target"` under `application.name:Melodia` |
| Next, onto a 44.1 kHz track (a reopen) | the new stream came up on the null sink at 44100 |
| Quit and relaunch | the new stream came up on the null sink |
| `pw-metadata -d <node> target.object` | back on the default sink; the stored target cleared |

**Windows, decided 2026-10-02 (Kenan): one device for both modes.** The existing `output_device`
key also routes shared output on Windows, in place of a separate `output_shared_device`. So Make
Bit-Perfect claims the device already playing, and a refused claim falls back to shared on that same
device. The cost: anyone on Windows who already picked an exclusive device hears shared output there
after updating.

**As built, Windows half** (static gates green on Windows, 2026-10-02).

**The seam**
- It gains `SHARED_DEVICE`: true for WASAPI, whose endpoint ids are the ones cpal opens by (both
  are `IMMDevice::GetId`); false for ALSA, whose `hw:` names would go round the sound server; false
  for the unsupported backend. It is surfaced as `SHARED_DEVICE_SUPPORTED` and re-read by
  `library::playback`.
- The `alsa.rs` line was written blind on Windows, for CI's Linux jobs to check.

**The request**
- `OutputRequest::Shared { rate, device }`, where the device is one of `devices()`' ids.
- `OutputChoice::shared_request` fills it from the choice, and only where `SHARED_DEVICE_SUPPORTED`
  holds, so a Linux card's id never reaches a shared open.
- Both `wanted_request` and `set_output_choice`'s nothing-loaded arm use it. `serves` needed
  nothing, since it compares requests whole.

**The open**
- `device::named_target` finds the id among cpal's `output_devices`, which is platform-neutral.
  It answers `None` only where no active device has that id. A listing that fails, or a device
  that is listed but can't name its config, is an error, because the reclaim poll would otherwise
  find it listed and reopen it every second. That is also why it doesn't call cpal's
  `device_by_id`, which answers `None` for a failed listing (found in review, 2026-10-02).
- `AudioOutput::open_shared(rate, device)`:
  - a device that refuses to open, held exclusively by another app for example, logs at debug and
    plays on the default;
  - a device that isn't connected logs once at info, plays on the default, and comes back marked
    `Named::Missing`.
- `start` folds that into `awaiting_device` beside a `NotConnected` claim, so the existing reclaim
  poll returns the stream to the device when it is plugged back in. `disconnected_device_returned`
  needed no change, since it already compares `choice.device` against `devices()`.
- A stand-in raises no `Fallback`, no toast and no verdict, since shared output already grades
  Converted. The Device row names where the audio went, and the picker reads "Not connected".
- `fall_back` opens shared on the refused claim's own device. Where that device isn't connected it
  goes straight to the default, and on Linux it passes none, as before.
- A stream on a named device is cpal's `Specific` handle, so it ignores changes to the Windows
  default. An unplug arrives as `DeviceNotAvailable`, which the error callback already reads as a
  lost device.

**Boot.** `open_output` parks where a shared device is chosen, so `hydrate_audio_dsp`'s
`set_output_choice` opens it directly rather than waking the default first.

**The picker**
- On Windows the row shows in both modes, as "Output Device" with the description "The sound card
  Melodia plays through".
- It leads with "System Default", which stands for no saved device. That makes following the
  Windows default reachable again; before, picking the first row saved the default endpoint's id.
- The label comes from a `pure callback` on `Settings`, since the rest of the list is Rust's.
- `Picked.default_label` holds that row as it reads on screen, and `pick_device` offsets by it.
  The options are rebuilt when the devices or the label change.
- Linux is unchanged: "Exclusive Device", no lead row.
- Three new strings, in all six catalogues.

**Moved on the way.** `claim_serves` moved to `claim.rs`, which keeps `mod.rs` under 800 lines.
`mod_tests` imports it from there.

**Tests adapted:** `output_settings_tests`' `picked` helper and its `take_listing` calls take the
new field and argument, passing `None`, so every row keeps its meaning.

**In-app run** (2026-10-02, debug build, Windows, scratch data folder). The window was driven by
clicks. Where the audio went was read off each endpoint's peak meter (`IAudioMeterInformation`),
alongside the log.
- An unplug was a disable of the endpoint through `IPolicyConfig::SetEndpointVisibility`, as the
  Sound control panel's Disable does.
- The Windows default was moved through `IPolicyConfig::SetDefaultEndpoint`, and restored after.

| Step | Result |
|---|---|
| Shared, nothing saved | the row reads "Output Device", "System Default" lit; played on the Elegiant, the default |
| The list | "System Default", then the two active endpoints |
| Pick the UMC22 | "Output reopened for a new output choice" on the UMC22; its meter moved, the Elegiant's read zero; the id persisted; the Device row named it |
| Windows default to the UMC22, then back | no reopen; the stream stayed on the UMC22 throughout |
| Pick System Default | `output_device` saved as `null`; played on the default, and followed two default changes ("output lost; reopening" onto each) |
| Pick the UMC22, then disable it | "the chosen output isn't connected, playing on the system default"; the picker read "Not connected"; the Device row named the Elegiant |
| Enable it again | "the chosen device is listed again; reopened" on the UMC22, 0.7 s later |
| Relaunch with the UMC22 saved | "parked until the chosen device opens", then one open on the UMC22, with no default open first |
| Mode to Exclusive, then back | claimed the UMC22 at S16 48 kHz, chip Bit-perfect; shared again on the UMC22 |
| Make Bit-Perfect from shared | the dialog, then Switch: claimed the UMC22; mode and device persisted |
| A 24-bit file under exclusive | refused as `FormatRefused`, played shared on the UMC22, not the default |
| A 96 kHz file, "Play Through the System Mixer" | refused as `RateRefused`, played shared on the UMC22 |
| Disable the UMC22 under a claim | refused as `NotConnected`, shared on the default with no second lookup; re-claimed exclusive once it was back |
| "Allow exclusive control" cleared (Phase 9's run) | refused as `NotAllowed`, played shared on the UMC22 |

No warning or error was logged apart from the expected refusals, and every quit was clean.

**Still open**
- A physical unplug of the UMC22. The run disabled the endpoint, which is the same state change
  for every reader here, but a USB removal goes through `NOTPRESENT` rather than `DISABLED`.
- Tests:
  - `take_listing` and `pick_device` with the lead row;
  - `shared_request`'s Linux filter;
  - `SHARED_DEVICE_SUPPORTED` in `mod_tests`' per-platform capability pins.
- Docs: done. The README's Bit-perfect output section has the device bullet and the fallback's
  device, and `.claude/rules/audio-stack.md` names `SHARED_DEVICE` and the shared stream's device.

## Phase 7 - File reads on the writer thread: measure, then decide

**Dropped** (Kenan, 2026-10-01). Not taken on; the design below is kept only for the record.

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

**As built, 8a, 8b and 8d** (static gates green). 8c is below, built on the Windows machine. All
three are platform-neutral, so exclusive output on Windows gets them too; CI's `clippy-windows`
and `test-windows` check them, and the Windows run closes this phase.

**As built, 8c** (static gates green on Windows, 2026-10-02)
- After each write, `Session::play` reads the device's clock, which it already did for the lead.
  The reading also carries the performance counter at the moment the device read its position.
- `stood_still` compares two consecutive readings. Where the device's clock advanced less than the
  wall clock, by more than `STALL_TOLERANCE` (2 ms), the device sat with nothing to play, and the
  write counts one `record_xrun`. That is the same counter ALSA's writer feeds.
- It is not asked before the clock first moves, since a stream's first period passes before it
  does.
- It shows only in `tasks::audio_health`'s debug line, every 5 s. cpal raises no `Xrun` for a
  WASAPI render stream, so before this Windows counted no underruns in either mode.
- **In event mode, a stall the device doesn't recover from restarts the stream** (`Session::restart`:
  stop, reset, prime, start), and the writer starts its count and readings over against the reset
  clock. The run below is why. A polled stream is left alone, since it tops up whatever room it
  finds.
- The clock arithmetic (`ClockReading`, `StallWatch` over `stood_still`, the tick and 100 ns
  conversions) lives in `output/wasapi_clock.rs`, Windows only, which keeps `wasapi.rs` under 800
  lines.

**The first design was wrong, and the in-app run showed why.** It compared the clock with the count
of frames written. Measured with temporary instrumentation through a one-second process freeze:
- Both devices hold their clock still while starved. The UMC22 moved 9 ms over the freeze, the
  ALC897 7 ms.
- The ALC897 halts with samples still queued, so the old check never saw that stall.
- The UMC22 comes back with less queued than before, so the old check counted about 4 false
  underruns a second for as long as the stream lasted.
- Clock against counter, in steady playback, stayed within ±0.75 ms on the UMC22 in both drive
  modes, which is what the 2 ms tolerance stands on.

**In-app run** (2026-10-02, debug build, Windows, scratch data folder, exclusive, volume 5 to 50).
The freeze was `NtSuspendProcess` for one second.

| Device, drive, period | Steady playback | The freeze | After it |
|---|---|---|---|
| UMC22, events, 5 ms (224 frames) | 0 over 2 min | counted | about 110 a second, without end |
| UMC22, events, 20 ms | 0 | counted | about 40 a second, without end |
| UMC22, polling, 5 ms and 20 ms | 0 | 2 | 0 |
| ALC897, events, 5 ms | 0 | 1 | 0 |
| ALC897, polling, 5 ms | 0 | 1 | 0 |

No run counted anything at the stream's start.

**Found by it: event mode on the UMC22 never recovers from a stall.** This predates 8c; the
counter only makes it visible.
- After the freeze, the writer's wakes drop to about half the rate, and the device runs dry every
  period until the stream reopens. The writer keeps reading the clock throughout, which is what the
  counter sees.
- The likeliest cause is the endpoint's auto-reset event: signals that land while the writer is
  stalled coalesce into one wake, so one buffer of the pair is never refilled.
- **Listening pass** (Kenan, 2026-10-02, UMC22 at volume 50): event mode was a clean tone before
  the freeze and buzzy after it. Polling mode was clean after the freeze.
- The ALC897 recovers in event mode. This is likely the stutter some USB drivers show under events,
  which the Polling Mode row exists for.

**Fixed: the restart above.** Re-run after it, through the same one-second freeze:

| Device, drive, period | The freeze | After it |
|---|---|---|
| UMC22, events, 20 ms | 1 | 0 |
| UMC22, events, 5 ms | 1 | 0 |
| ALC897, events, 5 ms | 1 | 0 |

**Listening pass** (Kenan, 2026-10-02, UMC22, events, 20 ms, volume 50). A generated arpeggio
(C–E–G–C over a C3 hum, `melody_48k.wav`) played clean after the freeze, where the sine had buzzed.

**Hardened after review** (Kenan, 2026-10-02). As run above, every stalled reading restarted the
stream. The 2 ms tolerance stands on two devices, and a driver whose clock steps coarser would
trip it on single readings, paying a period of silence at each. So `StallWatch` now restarts only
at `STALLS_WHEN_STUCK` (2) stalled readings in a row, and still counts every one as an underrun.
The UMC22's state after a freeze stalls at nearly every reading, so it still restarts. The ALC897
recovers by itself, so it no longer needs a restart at all.

Re-run (2026-10-02, the same one-second freeze):

| Device, drive, period | The freeze | After it |
|---|---|---|
| UMC22, events, 20 ms | 2 | 0 |
| UMC22, events, 5 ms | 2 | 0 |
| ALC897 at 7.1, events, 5 ms | 1 | 0 |

The ALC897's single count shows it never restarted, since a restart takes two stalled readings in a
row and both are counted.

**Listening pass** (Kenan, 2026-10-02, UMC22, events, 20 ms, volume 50, the arpeggio): smooth after
the freeze.

Should a device turn up whose stalled readings come interleaved with clean ones, the criterion wants
to become a count over a window rather than a run.

**8a**
- In shared mode where exclusive exists, the button shows whenever something plays, since the
  shared output stage always converts.
- One press can't take the device silently. Hardware Volume ships off, and exclusive output
  skips the system mixer, so with the system volume at 40% one press would hit the DAC about
  24 dB louder. So the press opens the existing confirm `Dialog` (kind `bit-perfect-exclusive`),
  which names both costs.
- Confirming writes the chip's index and calls `Settings.output-switch-to-bit-perfect`. That
  moves the `Picked` shadow, so the next pick can't revert the mode, then claims and resets in one
  blocking task. The reset reads off the claim whether the device carries the volume. Run as two
  tasks, the reset could land first, and a claim on a device a release earlier in the run put back
  then opens at full volume, `HardwareVolume::take` treating that device as resuming.
- Slint: `output-section.slint`, the dispatcher in `globals/dialog.slint`, the dialog's icon. Rust:
  the switch in `output_settings.rs`, sharing `signal_path.rs`'s repaint of the reset.
  Three new strings, in all six catalogues.

**8b**
- `SourceFormat` gains `lossy`, set in `decode::open` from the codec through `lossy_codec`.
  - That function is an explicit list of the lossy codecs `CODECS` opens: MP1, MP2, MP3, AAC,
    Vorbis, Opus, the three ADPCMs, A-law and μ-law.
  - Symphonia carries no such flag, and `AudioCodecId`'s field is private.
- The voice publishes it with the other source atomics.
- A lossy source grades the Source stage Converted, and `Verdict::Lossy` becomes the headline,
  after a fallback. The Device row keeps its own true fact.
- The UI:
  - verdict index 4, labelled "Lossy";
  - the Source row reads "{}, decoded from a lossy codec";
  - the chip's dot is red, as an MP3's already was.
- That row also corrects ADPCM, which read "rounded to fit 32-bit float" for a 32-bit integer
  container holding 16-bit samples.
- The claim ladder, `carries` and `claim_serves` ignore the field, so no new reopens.
- Three new strings, in all six catalogues.
- Tests adapted: every `SourceFormat` literal takes `lossy: false`. `file_decode_tests` expects
  `lossy` on its Ogg and MP3 fixtures.

**8d** differs from the design above. Parking with the decks still loaded leaves a state no path
handles:
- `Voice::clear` waits out `SERVICE_TIMEOUT` per voice with no callback to service it.
- Queued seeks mount only the first.
- A ninth queued op is dropped.
- Make Bit-Perfect would read a parked claim as software volume and raise the device on the
  reclaim.

So the long pause takes the track off the deck instead, the way a station's pause does:
- **Release.** The monitor's `PauseWatch` counts paused ticks while
  `PlaybackEngine::holds_releasable_claim` holds (the setting on, exclusive chosen, the output not
  parked). At `RELEASE_AFTER_PAUSE_MS`, five minutes, it calls `build_release_actions`.
  - That builder re-verifies what the monitor saw: still paused, same track, same position.
  - It then writes the position the deck stopped pulling at (everything before it has played
    out) and returns `Stop { fade_ms: 0 }`.
  - That stop clears the decks while the stream can still service it, then parks.
- **The chip.** The monitor publishes no signal path for a paused, parked output, so the chip and
  the button hide.
- **Play.** The state stays Paused. `library::playback`'s play and toggle ask the engine
  `holds_source()` under the state lock. A paused track its deck no longer holds takes
  `build_replay_actions`: the track starts again from its position, reclaiming through
  `reopen_for_track`. That is the path `resume_from_stopped` takes, and both now share
  `replay_current_track`.
- **Seeks.** A seek while released moves the position only, since `PlaybackEngine::seek` already
  returns on an empty deck.
- **No state flag.** The predicate is the deck, not a flag on `PlayerState`, which clippy's bool
  cap also rules out. So the rule is general: play on a paused track with nothing seated starts
  it, however it got there.
- **Where it lives.** `backend/mod.rs` is untouched but for moving its two position queries to a
  new `backend/reads.rs`, beside `holds_source`.
- **The setting.**
  - Persisted as `output_paused_device`, `"keep"` (the default) or `"release"`. It is a token
    because `OutputFlags` is at clippy's bool cap.
  - The row "Give the Device Back When Paused" sits under Hardware Volume, exclusive only.
  - It stays off `OutputChoice`, so a toggle reopens nothing.
  - Two new strings, in all six catalogues. The constant's doc names the figure the row's words
    spell.

**In-app run** (2026-10-01, debug build, dev data folder backed up and restored). Exclusive
runs went to the silent PCM2902 (`hw:CARD=CODEC,DEV=0`, Hardware Volume on), watched through
its `hw_params`. Shared runs went to a temporary null sink. Each step was driven over MPRIS,
or clicked through `ydotool`, and read off window captures.

| Item | Check | Result |
|---|---|---|
| 8b | 48 kHz 16-bit FLAC on the claim | chip "Bit-perfect · 48 kHz", every stage green |
| 8b | 48 kHz MP3 on the same S16 claim | chip "Lossy · 48 kHz"; Signal Path "Lossy: the file is compressed…"; Source "48 kHz · 32-bit float · 2 ch, decoded from a lossy codec"; Device still "…can't hold the source exactly" |
| 8d, on | MP3 paused at 20:31:46 | card closed at 20:36:46.857 (`player: stop (fade 0ms)`); player still Paused on the track; the sound server took the card back |
| 8d, on | `paplay` to the card's sink while released | played (exit 0) |
| 8d, on | `SetPosition` to 8 s while released | position 8.0 s, still Paused, card still closed |
| 8d, on | `Play` | `play … from 8000ms`, claim reopened at S16_LE 48000 |
| 8d, on | second pause, then `PlayPause` (the media key's and the bar button's path) | released at 20:43:30.473, five minutes on; replayed `from 25440ms` and reclaimed |
| 8d, off | 5.5 min paused | card held throughout, no release |
| 8a | shared, FLAC playing | "Make Bit-Perfect" shown beside "Converted…" |
| 8a | press it | the dialog, with its text, icon, Cancel and Switch |
| 8a | Cancel | dialog closed; `output_mode` still `shared`, volume 10, card untouched |
| 8a | press it, then Switch | chip on Exclusive, exclusive rows shown, Signal Path and chip Bit-perfect, `output_mode: exclusive` persisted, claim at S16_LE 48000. The volume stayed at 10, Hardware Volume carrying it |
| 8a | pick Buffer Period 10 ms afterwards | `output_mode` still `exclusive`, `output_period_ms: 10`, period 480 frames |

No warning or error was logged in any run, and each quit through the tray handed the card back.

Not exercised: Next or Stop while released (both land on paths an empty deck already takes), and
8b on shared output, where the chip is hidden.

Found on the way, not touched: MPRIS's relative `Seek` is ignored ("needs library API
support"), so only `SetPosition` moves the position from a media panel.

**Windows run** (2026-10-02, debug build, UMC22 exclusive, Hardware Volume on):

| Item | Check | Result |
|---|---|---|
| 8a | Make Bit-Perfect from shared, then Switch | the dialog, then a claim at S16 48 kHz on the same device shared output was playing through; mode and device persisted |
| 8b | a 192 kbps MP3 on the claim | chip "Lossy · 48 kHz", red dot; Signal Path "Lossy: the file is compressed, so the device gets the decoder's reconstruction of the recording"; Source "48 kHz · 32-bit float · 2 ch, decoded from a lossy codec"; Device "…in a format that can't hold the source exactly" |
| 8d, on | paused at 17:11:18.558 | `player: stop (fade 0ms)` at 17:16:18.088; still paused at 0:38; chip hidden, Signal Path "Nothing is playing" |
| 8d, on | Play | `play … from 38680ms`, claim reopened at S16 48 kHz |

**Still open**
- Tests, none written:
  - `stood_still`'s table: the tolerance's edge, a clock that hasn't started, a counter that went
    backwards;
  - `StallWatch`: one stall reads `Stalled`, two in a row `Stuck`, and a clean reading between them
    starts the run over;
  - `lossy_codec`'s table;
  - the Lossy verdict rows;
  - `PauseWatch`;
  - `build_release_actions`' re-verify;
  - `build_replay_actions`, and play and toggle routing on an empty deck;
  - `PausedDevice`'s round-trip.
- Docs: done. The README's Bit-perfect output section names the Lossy verdict and the long
  pause's release, and `.claude/rules/audio-stack.md`'s verdict and exclusive bullets carry both.

## Phase 9 - Gapless across a rate change the device doesn't see

**The problem.** Measured on 2026-10-02 on the UMC22, with Resample and slices of one continuous
sine:
- 96 → 48 kHz, 48 → 96 kHz and 88.2 → 44.1 kHz each refused gapless staging and reopened the
  device, at the rate it was already running. A 48 → 48 kHz pair stayed gapless.
- Each reopen took 145 to 205 ms. With the monitor's tick before it and the new claim's priming
  after it, the seam costs roughly 0.25 to 0.8 s of silence.

ALSA behaves the same, since both backends go through `claim_serves`.

**Why.** `claim_serves` reuses the open claim only for a request that differs from the open one
in the source's format. A different source rate is a different request, whatever the device would
run at for it.

**The question both approaches answer:** would a fresh claim for the next track run the device at
the rate it runs now, with everything else `claim_serves` already checks still holding? If so, the
open claim plays the track, through the voices' converter where the rates differ, and the seam is
gapless.

**Two approaches, tried in turn.** 9B extends 9A rather than replacing it. Both were built and run
on 2026-10-02, and **9B was kept**, with 9A's rule living on inside it as the answer where no
offered set is known. The WASAPI half is built; the ALSA half is open, for the Linux machine (see
Still open).

### 9A - Serve a track at the rate the device already runs

- `claim_serves` (`output/mod.rs`) takes the device's rate (`Negotiated.shape.rate`) beside its
  format. It compares requests ignoring the source's rate as well as its format.
- It serves where the format check holds and the next source's rate is either the open request's
  (as today) or the device's.
- That is exact rather than a guess: a fresh claim tries the source's own rate first, and the
  device is running that rate now.
- Under "Play Through the System Mixer" the device always runs the open request's rate, so nothing
  changes there.
- `AudioOutput::serves` passes the rate from the `Negotiated` it already reads. There is no new
  state, no backend code and nothing platform-specific.

**What 9A leaves.** A served track doesn't replace the open request. So once a claim was opened
for a 96 kHz track, every 48 and 96 kHz track after it is served. The gap that remains is the first
96 kHz track after a claim opened for a 48 kHz one, where the device runs the source's own rate.
A fresh claim for 96 kHz would pick 48 kHz there, but 9A can't know that without knowing what else
the device offers. So 9A leaves one gap per claim that starts at the device's own rate, rather
than one per seam.

### 9B - Remember which rates the device offered

**Pieces**
- **`RateSet`** (`rates.rs`): the `LADDER` rungs a device took, as a bitmask. The 15 rungs fit a
  `u16`, so `Negotiated` stays cheap to clone. Its `Debug` lists the rates, so the log's
  `Output reopened` line shows them.
- **`rates::claim_rate(source, offered, fallback) -> Option<SampleRate>`**: the rate a fresh claim
  runs the device at.
  - The source's own rate where the device offers it.
  - Otherwise, under Resample, `device_rate_for` over the rest.
  - Otherwise none.

  On WASAPI, `negotiate` keeps its own decision and `claim_rate` only predicts it, since a fresh
  WASAPI claim decides per layout, then falls back to the mix rate. On ALSA, `pick_rate` is
  `claim_rate` over the card's set and can become a call to it (Still open).
- **`Negotiated` gains `offered: Option<RateSet>`.** An exclusive claim under Resample sets it.
  It is `None` everywhere else: shared streams, the Shared fallback, the unsupported backend, and
  for now ALSA. The `Negotiated` literals in `alsa.rs`, `wasapi.rs`, `device.rs`,
  `signal_path_tests.rs` and `mod_tests.rs` each gained the field.
- **`claim_serves`** serves where `claim_rate` of the next source's rate is the device's rate.
  Where `offered` is unknown, it keeps 9A's rule.
- `mod rates` moves out of `cfg_select!` to the top of `mod.rs`, since `claim_serves` is
  platform-neutral and now calls it.

**The probe on ALSA.** Open, for the Linux machine. The steps are under Still open.

**The probe on WASAPI** (`offered_rates`, called from `negotiate`). Under Resample, the probe runs
before anything initialises. So whether `IsFormatSupported` still answers while our own exclusive
stream holds the endpoint never matters, and was not checked.
- **The set has to be a superset of what a fresh claim for any next track could see.** Otherwise
  9B would keep a track converted that a fresh claim would have played at its own rate. For
  example: a device taking 96 kHz only at 24 bits is running S16 at 48 kHz, and a 16-bit 96 kHz
  track comes next, whose ladder includes the 24-bit rungs.
- So a rung counts as offered where the device takes **any** layout WASAPI can declare, at any
  channel count `candidates` would ask, through `takes_exclusive`, respellings included. That is
  `takes_at` with `ANY_LAYOUT`, the F32 source, whose ladder is every rung.
- **Why the superset is safe:**
  - A serve needs the next source's rate to be missing from the set. A fresh claim can't play it
    at its own rate either.
  - A serve also needs the device's current rate to be the policy's best pick from the set.
  - The fresh claim picks from a subset that still holds the current rate, since `claim_serves`
    checks that the running layout is on the next format's ladder. So its best pick is that rate
    too.
  - Where the superset is wider than the fresh claim's set, 9B reopens as today. It never serves
    where a fresh claim would choose another rate.
  - **The one divergence found:** a device taking none of the next track's layouts at its own mix
    rate. There a fresh claim stops at `FormatRefused` and plays shared, where 9B keeps the claim
    at the rate the device runs. That errs towards staying exclusive, and no real device is known
    to do it.
- **Cost:** `IsFormatSupported` calls once per claim under Resample, before the device opens, up
  to 15 rates × every declarable rung × every channel count `candidates` spans (the source's up to
  the device's own) × each rung's respellings. A rate the device takes stops at the first layout
  it takes, which on the UMC22 is still S16 after four refused rungs, so the rates it lacks are most
  of the cost. Measured below at about 35 ms on the UMC22, a 2-channel device, where a missing
  rate costs 17 calls: a short header for S16 and F32, and two channel masks for every rung. A 7.1
  endpoint playing stereo also asks 3 to 8 channels, where `make_channelmasks` offers three or
  four masks a rung, so 162 calls per missing rate, about ten times as many.

**What stays the same, under both**
- The decks, the converter and the verdict.
  - A served track at the device's rate plays at step 1, so the chip reads Bit-perfect.
  - A served track at another rate reads Converted, as the claim's first track did.
- The seam itself.
  - Tracks of different source shapes don't hand the converter over
    (`VoicePull::hand_over_or_drain`). The outgoing track drains against silence and the next
    starts fresh.
  - So there is no gap, but there may be a sub-millisecond taper either side of the seam.
  - Shared output already plays every cross-rate gapless seam this way. The ear check listens for
    it; this phase doesn't change it.
- Crossfade between such tracks. `reopens_for` asks the same `serves`, so under exclusive output
  they fade where they used to cut.
- A device-loss reopen. It still asks for `AudioOutput::request`, the open claim's own request,
  which lands on the same device rate.

**As built** (static gates green on Windows, 2026-10-02). Where it differs from the plan above:
- `claim_serves` takes the claim's whole `Negotiated` rather than its format and rate, so the
  shared-stream case is its own early return. Its two tests were adapted to that, with a
  `running(format)` helper; no new rows.
- `negotiate` is split. The outer half opens the probe client and, under Resample, asks for the
  offered set. `open_session` holds the two attempts, unchanged.
- `RateSet::contains` answers `None` for a rate off the ladder, and so `claim_rate` does too. A
  24 kHz MP3 has no bit in the set, so the set can't say whether a fresh claim would run the device
  at that rate itself, and 9A's rule decides alone. Found while writing this up, after the run;
  every fixture rate is on the ladder, so the run below is unaffected.
- Under Resample, `negotiate` asks the claim's first candidate through `exclusive_spelling` ahead
  of the probe, so a busy or barred device refuses in one call rather than after the whole sweep.
  `retry_at_track_start` retries both refusals at every track start, so a device with exclusive
  control barred in Windows would otherwise pay the sweep on every track. Found in review, after
  the run.

**In-app run** (2026-10-02, debug build, Windows). The UMC22 ran exclusive with Resample, polling
and Hardware Volume on. Each queue was a fresh launch. The fixtures were slices of one 440 Hz sine
at −20 dBFS, cut at whole samples so the tone runs on across every seam. The Today column is the
earlier same-day run where it measured that seam, and the unchanged code's answer elsewhere.

| Queue | Today | 9A | 9B |
|---|---|---|---|
| 96k → 48k → 48k → 96k | 2 reopens, at 48000 | one claim, gapless | one claim, gapless |
| 48k → 96k → 48k | 2 reopens, at 48000 | 1 reopen into the 96k, then gapless | one claim, gapless |
| 88.2k → 44.1k | reopen, at 44100 | gapless | gapless |
| 44.1k → 88.2k | reopen, at 44100 | reopen, at 44100 | gapless |
| 96k → 44.1k (must reopen) | reopen | reopen | reopen |
| 96k → 88.2k (must reopen) | reopen | reopen | reopen |
| 48k → 96k under "Play Through the System Mixer" (must not serve) | refused, played shared | the same | the same |
| 96k → 48k, crossfade on | cut and reopened | faded (1573 ms) | faded (1610 ms) |

- 9B logged `offered: Some([32000, 44100, 48000])`, the UMC22's three rates, on every Resample
  claim, and `None` under "Play Through the System Mixer".
- A 48 kHz track served on the claim opened for 96 kHz read "Bit-perfect · 48 kHz".
- Claim latency, from play to `Output reopened`:
  - a first claim, from a parked output, took 15 to 44 ms under 9A and 50 to 74 ms under 9B;
  - the two reopens went from 180 to 210 ms and from 203 to 229 ms;
  - so the probe costs about 35 ms per Resample claim. A claim under "Play Through the System
    Mixer" probes nothing and took 20 ms either way.
- No warning or error was logged apart from the expected refusal, and every quit was clean.

**Listening pass** (Kenan, 2026-10-02, 9B). On the UMC22, the 48 → 96 → 48 kHz slices played as
one unbroken tone, with no gap or tick at either seam.

**Chosen: 9B.** It is the only one that closed every same-rate seam, its probe costs about 35 ms
per Resample claim, and it falls back to 9A's rule wherever no offered set is known.

**The probe where it costs most** (2026-10-02, debug build, Windows). Each figure runs from the
play line to `Output reopened`, three fresh launches each:

| Device, layout | File | "Play Through the System Mixer" | Resample (probe) |
|---|---|---|---|
| UMC22, stereo | 48 kHz | 20 ms | 52–56 ms |
| UMC22, stereo | 96 kHz, converted to 48 | | 71–81 ms |
| ALC897, stereo | 44.1 kHz | 10 ms | 135 ms |
| ALC897, **7.1** (Kenan set it in Speaker Setup, then back) | 44.1 kHz | 10 ms | **1,273–1,377 ms** |
| ALC897, **7.1** | 96 kHz | | 1,015–1,041 ms |

- The probe already costs the ALC897 about 125 ms in stereo, since it lacks 11 of the 15 rungs. A
  7.1 layout asks every channel count from 2 to 8 at each of them, which is the "about ten times"
  the cost note above predicted.
- Under 7.1 the claims still took the stereo layout, and offered the same four rates.

A refused claim, with "Allow applications to take exclusive control" cleared on the UMC22 (Kenan
cleared it, then set it again):

| Unsupported Sample Rates | Play to the shared stand-in |
|---|---|
| "Play Through the System Mixer" | 28–34 ms |
| Resample | 32–34 ms |

So `negotiate`'s first ask refuses a barred device in one call, ahead of the sweep, matching 9A's
refusal. The stand-in played shared on the UMC22 itself (Phase 6).

**Still open**
- **Linux: 9B's ALSA half.** Until it lands, ALSA fills no offered set and Linux has 9A's
  behaviour: a 96 kHz track after a claim opened for a 48 kHz one still reopens. On the Linux
  machine:
  1. **`alsa.rs`, `pick_rate`.** Under Resample, test every `rates::LADDER` rung with `test_rate`
     on the same `HwParams`, and collect the answers into a `RateSet` rather than the `Vec` the
     refusal path builds today. Do it whether or not the source's rate is refused, since a later
     track needs the set either way. Keep testing the source's own rate first and directly: a rate
     off the ladder has no bit in a `RateSet`, so `claim_rate` alone would refuse a rate the card
     runs.
  2. **The pick.** Keep `device_rate_for` over the set's rates, or call `rates::claim_rate`, which
     is the same policy.
  3. **Carry the set** through `Config` into `Negotiated.offered`, replacing the `offered: None` in
     `open`'s literal. That line was written blind on Windows, so the first Linux build is also
     what checks it compiles.
  4. **No superset argument is needed.** `test_rate` on that `HwParams` ignores format and
     channels, and it is the very set a fresh `pick_rate` decides from. So the prediction is
     exactly the fresh claim's decision.
  5. **Tests:** adapt `alsa_tests`' configure rows if `Config` changes shape.
  6. **Run the same eight queues** on the UMC22 (`hw:CARD=CODEC,DEV=0`) with Resample. Expect the
     9B column, and the card's rates in the log's `offered`. Note the claim latency too.
     - Fixtures: `sine=frequency=440:duration=16` at each rate, cut with
       `atrim=start_sample=…:end_sample=…` at 0, 5.31, 7.31, 10.62 and 16 s. Each cut is a whole
       sample at every rate used.
     - Queues, in order: 96k a → 48k b → 48k a → 96k b; 48k (0 to 5.31 s) → 96k (5.31 to 10.62 s)
       → 48k (10.62 to 16 s); 88.2k a → 44.1k b; 44.1k a → 88.2k b; 96k a → 44.1k b; 96k a →
       88.2k b. Here "a" is 0 to 7.31 s and "b" is 7.31 to 16 s.
  7. **Then the README.** Extend the Unsupported Sample Rates bullet with "and an album mixing
     such rates stays gapless wherever the device's own rate doesn't change". It waits for the
     ALSA half, since until then that is true on Windows only.
- **Windows: the probe's cost on a 7.1 output, built and not yet run.** The run that measured it
  is above. What was found:
  - **Two sweeps, not one.** `offered_rates` asked every rung in every layout. Where the source's
    own rate was refused, as with an 88.2 kHz file on the ALC897, `device_rate` then ran a second
    sweep with the source's formats. That case was never timed, but it should have cost both.
  - **A rate the device lacks is the expensive answer.** It is only known once every channel count,
    rung and respelling has been refused. The ALC897 lacks 11 of the 15 rungs, and 7.1 asks 2 to 8
    channels at each.
  - **Nothing cheaper keeps the guarantee.** Narrowing the channel counts or dropping the
    respellings would make the set narrower than a fresh claim's. An HDMI sink that takes
    192 kHz in stereo and not in 7.1 is the real case that breaks.

  As built:
  - **`wasapi_offered::remembered`** keeps each sweep's answer for the process's life, keyed by
    exactly what the sweep asks with: the endpoint id, the source's channel count it starts from,
    and the device's own. So the first Resample claim on a device in a layout pays the sweep, and
    every later one reads it.
  - A change in Speaker Setup moves the device's channel count and asks again. The cache holds at
    most 16 answers.
  - The sweep logs its time at debug ("asked … for its rates in …").
  - **`device_rate` is gone.** `open_session` picks the retry rate out of the offered set, less the
    source's refused rate. That set answers for any layout, so the pick can be a rate the source's
    own formats can't open. The mix rate stands behind it then, as for a driver that passes a rate
    and refuses to initialise it.
  - `RateSet::rates` is now `pub(super)`.

  Run (2026-10-02, debug build, the ALC897 at 7.1, Resample, from the play line to
  `Output reopened`):

  | Session, claim | Before | Now |
  |---|---|---|
  | First, an 88.2 kHz file converted to 44.1 kHz | about 1.3 s, plus a second sweep | 1,074 ms, 988 ms of it the one sweep |
  | Then a 48 kHz reopen | 1,015–1,377 ms a claim | 23.5 ms, no sweep |
  | Then a 44.1 kHz reopen | | 23.3 ms, no sweep |
  | Relaunched: first, 48 kHz | | 996 ms, the sweep again (985 ms) |
  | Then an 88.2 kHz reopen, converted to 44.1 kHz | | 128 ms, no sweep |

  - Every claim offered the same four rates.
  - The shared stand-in still worked: the UMC22 picked by id, disabled and enabled again, with the
    ALC897 standing in at its 8-channel mix.

  Still open:
  - **The first claim on a 7.1 output still pays one sweep**, at its first play in a session. A
    background sweep could hide that, but it would need `IsFormatSupported` to answer while
    Melodia's own exclusive stream holds the endpoint, which was never checked.
  - **Tests:** the remembered set answers a second key without probing; a key differing in the
    device's channel count probes again; the oldest of 17 is the one dropped.
- **Tests:**
  - `claim_serves` rows for both rules, including an off-ladder rate falling back to 9A's;
  - `claim_rate`'s table;
  - `RateSet`'s round trip through `LADDER`;
  - a pin that F32's ladder holds every `DeviceFormat` rung, since the WASAPI superset
    (`ANY_LAYOUT`) rests on it.

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
- Phase 3: whether `device_rate_for` should take a rate above the source, of either family, before
  dropping below it. On the Windows Realtek, which lacks 88.2 and 176.4 kHz, every 44.1 kHz-family
  hi-res file lands on 44.1 kHz while the device offers 96 and 192 kHz. The kernel converts any
  ratio equally well, so the family buys nothing audible there.
- Phase 6: the Linux mechanism. Decided 2026-10-01: none in the app, the sound server routes it.

## Verification

After each phase:

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked --workspace -- -D warnings
cargo test --locked --workspace
```

- **Tests that stay green:** `crates/melodia/tests/bit_perfect.rs`, `stream_rate.rs` and
  `crossfade.rs`, both `…through_an_output_reopen` cases included.
- **The listening is Kenan's.** The in-app runs are driven from here where he allows it, in a
  scratch data folder, and he is called in for whatever only an ear can answer.
- **At the end of Phase 2:** a release build's CPU at the capped worst case, and `/usr/bin/time -v`
  peak RSS.
