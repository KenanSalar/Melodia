# Bit-Perfect Output

Working doc. Delete when the feature ships.

Status: **proposed** · Created: 2026-08-14

> Upstream and local facts below were verified **2026-08-14** against the pinned
> `rodio 0.22.2` / `cpal 0.17.3` sources in the registry, this tree's `Cargo.lock`,
> and the rox checkout at `~/Development/rox` (ADR 9 / ADR 19,
> `crates/rox-playback/src/output/`).
>
> **Re-read against #90 before acting on any of it.** rodio is gone and
> `src/player/output/` exists, so findings 1 through 3 and Phase 1 are largely spent —
> each is marked where it changed. Everything about exclusive mode is untouched.
>
> **cpal has since moved to 0.18.2**, which changes three premises below: a built stream now
> stays paused until `play()`; the block size is still the host's choice but is finally
> *readable*, through `StreamTrait::buffer_size`, which `Negotiated` now carries beside the
> request; and a lost device arrives as `ErrorKind::DeviceNotAvailable` on ALSA too, so
> finding 3's "already has the signal" is now true on every host rather than inferred on
> Linux. Finding 4's three bindings all survived the bump.

---

## What ships

An **Exclusive / bit-perfect output** mode: Melodia claims the audio device for
itself, sets it to the *file's* sample rate, and hands the decoder's samples to the
DAC unmodified.

The claim has to be checkable or it is decoration. Melodia's is:

> With bit-perfect on, and the mode reporting **Engaged**, the samples the device
> receives are the decoder's output, bit-identical.

That holds only when **all** of these are true, and the UI states which one is not:

| condition | why it's load-bearing |
|---|---|
| Device claimed exclusively | otherwise the OS mixer resamples and mixes |
| Mixer rate == source rate | `Mixer::add` resamples every source to the mixer's rate (finding 1) |
| Mixer channels == source channels | a mono→stereo upmix is not the decoder's output |
| EQ off, ReplayGain off, limiter bypassed | each multiplies samples by design |
| Crossfade not running | a fade is two sources summed |
| Speed == 1.0 | anything else is resampling |
| Volume == 100% | see below |

The visualizer tap is the one thing that **stays on**: it is read-only and taps
before the deck's conversion, pause and volume (`src/player/CLAUDE.md`), so it observes
without touching. That's a Melodia advantage worth keeping — most players kill their
visualizer in exclusive mode.

Volume is the interesting one, and #90 settled it: a deck skips the multiply entirely
when its volume's bit pattern is exactly unity, which `volume_to_amplitude` produces at
volume 100. Bit-exactness is therefore structural rather than an f32 accident — but the
*user-facing* rule stays "volume at 100%", because that's the only value we'd defend,
and Phase 4's round-trip test is what pins it rather than the argument.

**Not in scope for v1:** DSD / DoP (no Rust DSD decoder), ASIO, sample-rate families
the device refuses, mono and multichannel sources (they report *not engaged* with the
reason "channel conversion"), and any form of upsampling.

---

## Prior art — what the robust ones agree on

Surveyed: foobar2000 (WASAPI component + ASIO), Audirvāna (hog mode + integer mode),
Roon / RoonBridge, HQPlayer, MPD's ALSA output, Squeezelite, MediaMonkey, Strawberry.

Six things every one of them does, and one thing only the good ones do:

1. **Claim the device exclusively** — WASAPI `AUDCLNT_SHAREMODE_EXCLUSIVE` on Windows,
   CoreAudio hog mode on macOS, ALSA `hw:` with resampling disabled on Linux. This is
   the whole feature; everything else is bookkeeping.
2. **Follow the source rate per track**, reopening the device when it changes, and
   eat the audible gap that costs. Nobody has solved gapless-across-rates; they all
   gap.
3. **Hard-bypass every DSP node**, including their own volume control, and disable
   the software volume slider outright.
4. **Report what was negotiated, not what was requested** — rate, bit depth, format,
   and whether the mode is actually engaged.
5. **Fall back to shared output with a visible reason** when the claim fails (device
   busy, rate unsupported). Never silence, never a dead toggle.
6. **Expose a buffer/period knob and a resync delay.** Roon calls it "Resync Delay";
   foobar calls it hardware buffer. Both exist because real DACs need time to relock
   their PLL after a rate change, and a fixed constant serves nobody.
7. **The good ones are event-driven where the OS offers it.** On Windows that means
   `AUDCLNT_STREAMFLAGS_EVENTCALLBACK` + `SetEventHandle` — the driver signals when a
   buffer is free instead of the app polling a timer. Microsoft's `RenderExclusiveEventDriven`
   sample is the reference, and in exclusive mode it forces `hnsPeriodicity ==
   hnsBufferDuration`. On macOS the HAL's IOProc is already a push. On Linux, blocking
   `snd_pcm_writei` is the equivalent.

One thing **none** of the desktop players handles well and Linux forces on us:
coordinating the `hw:` claim with the running sound server. WirePlumber reserves ALSA
devices over the `org.freedesktop.ReserveDevice1` D-Bus protocol; taking the card
without asking either fails or yanks it. See Phase 5.

---

## rox: what to take, what to leave

rox spent this option already (ADR 9 deferred it, ADR 19 spends it) and the
implementation is genuinely good — the design is right, the comments argue rather
than narrate, and the format ladder and period math are pulled above the FFI so they
compile and test on every platform. The negotiation code is the expensive half and
it is the half worth porting.

**Take:**

- The **shape**: one seam (`Request` → `Negotiated`), one shared fill point every
  backend calls, per-platform backends behind `cfg`, fallback-to-shared-with-a-reason.
- The **format ladder** (`f32` → `s32` carrying 24 valid bits → `s16`, best-first,
  a named format the device refuses falls back and the *result* is reported). Packed
  24-bit deliberately absent — no Rust type carries it and every card offering it
  offers `S32_LE`.
- The **WASAPI alignment dance**: `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` → `GetBufferSize`
  → recompute the duration with the half-tick round-up → **activate a fresh
  `IAudioClient`** (a failed one can't be re-initialized). Also `IsFormatSupported ==
  S_OK` exactly, never "not an error" — `S_FALSE` is the shared-mode near-miss answer.
- The **channel mask table** — an exclusive-mode driver compares the mask against what
  it publishes, so filling the low bits spells 4 channels as FL/FR/FC/LFE and the claim
  fails for a reason nothing reports.
- CoreAudio's **hog-mode toggle guard**: `AudioHardware.h` says the value passed to a
  hog-mode *set* is ignored and the set **toggles**, so setting while you already own
  it releases it. The read-back is what decides whether you got it.
- The **nominal-rate settle poll** — the set returns before the driver relocks, and
  reading straight back gets the old rate.
- ALSA's `set_rate_resample(false)` — the one line that makes the mode mean anything.

**Leave, and why:**

| rox does | Melodia should | why |
|---|---|---|
| WASAPI push-mode, high-resolution waitable timer, wake twice a period | event-driven: `AUDCLNT_STREAMFLAGS_EVENTCALLBACK` + `SetEventHandle` + `WaitForSingleObject` on the driver's event | the documented exclusive design; no timer drift, no wasted wakes, lower latency. rox's own comment notes the `buffer == period` constraint and routes around it rather than accepting it |
| writer threads at default priority | `AvSetMmThreadCharacteristics("Pro Audio")` on Windows; `SCHED_FIFO` (best-effort, degrade quietly) on Linux | a hand-rolled writer thread is exactly where the OS expects you to ask for RT scheduling. Without it a 40 ms buffer is at CFS's mercy |
| claims `hw:` with no coordination | acquire `org.freedesktop.ReserveDevice1` first, release on drop | on a stock PipeWire desktop — which is every Fedora/Ubuntu install — the card is already reserved. Without this, Linux exclusive fails for most users and the reason string blames the hardware |
| `fill()` pops the ring one sample at a time, re-reads `slots()` per frame, matches on channel count per frame | drain in contiguous chunks, hoist the channel decision out of the loop | this is the measurable inefficiency. `rtrb::read_chunk` hands back slices; per-frame atomic loads and a per-frame `match` on a value that never changes are pure overhead on the RT thread |
| bit-exactness argued in the ADR, tested only in `f32` | a round-trip test: known 16- and 24-bit fixtures → the full chain, bypassed → assert bit-identical | rox's hardware tests are `#[ignore]`d, so nothing in CI pins the claim. Melodia can pull `MixerPull` in a plain unit test and prove it without a device |
| `Result<_, String>` throughout | `AppError` with the boundary variants | the tree's error contract; `error::describe` needs a typed cause |
| hand-declares ~300 lines of CoreAudio `extern "C"` + four-char constants | use `objc2-core-audio` | finding 4 — it's already in our lock file, needs no SDK and no bindgen, and covers the whole surface |

Straight answer on "not optimized": the *design* is sound and the FFI is careful.
The real costs are the polled WASAPI loop and the per-sample ring drain, and the real
*gaps* are scheduling priority and the Linux reservation — those matter more than the
CPU does.

---

## Five findings that decide the shape

1. **Resampling is per source and visible, and the passthrough is exact.**
   `output::convert` runs per deck source rather than per voice, and steps exactly one
   source frame per output frame when the rates match — bit-identically, `-0.0`
   included, which `convert_tests` pins. **Bit-perfect still requires the mixer to run
   at the source rate**, since the conversion happens whether or not anyone wanted it;
   what changed is that following the rate is now a mixer rebuild rather than a fight
   with a dependency. *(Was: `Mixer::add` resampled inside rodio at add time against a
   rate fixed at construction. #90 removed that.)*

2. **A new mixer means new decks.** `Decks` takes its two voices off the mixer at boot,
   and that's the property crossfade rests on. Both must be rebuilt against the new
   mixer, under the decks lock, in the right order. This is the structural spine of the
   feature and the only genuinely risky part — hence its own phase, built and shipped
   against the *shared* backend where a mistake is a glitch rather than a dead device.
   The voices are reference-counted, so the rebuild is a swap rather than a lifetime
   problem.

3. **The reopen is unblocked.** *(Was: `MixerDeviceSink` was `Box::leak`'d so it
   outlived every `Player`, and a leaked sink never releases the device.)* #90 made the
   handle an owned `AudioOutput` on `AppState` with the stream dropping before the
   decks, and corrected the `process::exit(0)` paragraph in `CLAUDE.md` — which stays,
   because three other threads never exit and only one of the four was the sink's.

   **rox already ships the recovery half, so it need not be re-derived.** Its cpal error
   callback only sets a `device_lost` flag (`crates/rox-playback/src/output.rs:381-389`)
   and the UI pump polls it and calls `reopen_device`
   (`crates/rox-services/src/player.rs:1345-1357`), rebuilding the session at the current
   spot or clearing it with a reason. That is the shape `tasks::audio_health` defers here:
   it already has the signal and deliberately only reports it, because acting on it means
   the deck rebuild item 2 above owns.

   **Take the reopen, not the callback.** rox is on 0.18.1 and still classifies nothing: one
   `device_lost` store for every kind it receives, including the three 0.18 documents as
   non-fatal (`Xrun`, `RealtimeDenied`, `DeviceChanged`, the last saying outright that the
   stream stays active). So it rebuilds a whole session on an xrun. `stream_health`'s
   three-way split is the half to keep when the reopen lands on top of it.

4. **Every platform binding we need is already in `Cargo.lock`,** pulled by
   `cpal 0.18.2`:
   - `alsa 0.11.0` — Linux. `libasound` is already linked; a direct dep is a manifest
     line, not a new C dependency.
   - `windows 0.62.2` — Windows. **Pin exactly this version** so the two share one
     copy of the bindings instead of compiling a second set.
   - `objc2-core-audio 0.3.2` (+ `-types`, `objc2-core-foundation`) — macOS. Verified
     to expose `AudioObjectGetPropertyData` / `SetPropertyData`,
     `AudioObjectPropertyAddress`, `AudioDeviceCreateIOProcID`,
     `kAudioDevicePropertyHogMode`, `kAudioDevicePropertyNominalSampleRate`,
     `kAudioStreamPropertyPhysicalFormat`. Pure Rust, no SDK headers, no bindgen — so
     it type-checks from a Linux CI runner, which is the constraint that pushed rox
     into hand-declaring the lot.

5. **The rate is already in the database and not on the projection.**
   `migrations/20260514000000_initial_schema.sql:102–104` gives `tracks` its
   `channels`, `sample_rate` and `bit_depth` columns. `TrackSummary` doesn't carry
   them. ReplayGain solved exactly this problem — baked onto `TrackSummary` so the
   sync `play_track_inner` needs no async fetch — and the same argument applies
   verbatim: the reopen decision has to be made before the source is built.

---

## Structure

**The directory exists.** #90 built `src/player/output/` for the shared backend, so this is no
longer a greenfield layout — it is four files to extend rather than one `shared.rs` to write:

```
src/player/output/
  mod.rs        AudioOutput: the open handle, and the only door onto Mixer
  device.rs     the cpal stream, the config ladder, Negotiated, the period math
  mixer.rs      the unclamped sum, in LOCKSTEP_FRAMES steps
  voice.rs      one voice: transport, the command channel, the clock
  convert.rs    rate, channel map, the speed ratio
  tests/        convert / voice / mixer / device, per the tree's `#[path]` convention
```

What this plan still adds, on top of that:

```
  mod.rs        + the seam: OutputMode, OutputRequest, OutputBackend, pump()
                  (Negotiated is already device.rs's and moves or is re-exported)
  device.rs     becomes the shared backend behind the trait
  alsa.rs       #[cfg(target_os = "linux")]
  reserve.rs    #[cfg(target_os = "linux")]  org.freedesktop.ReserveDevice1
  wasapi.rs     #[cfg(target_os = "windows")] — format ladder + period math above the cfg
  coreaudio.rs  #[cfg(target_os = "macos")]
```

Ownership rules, so this doesn't sprawl:

- **`pump()` in `mod.rs` is the only place samples reach a device buffer**, and every
  backend calls it. Two backends drifting on what they do to the samples would make
  "bit-perfect" mean two things. Same argument as rox's `fill`, same enforcement.
- **`stream_health.rs` stays where it is** and every backend reports through it, so
  `tasks::audio_health`'s Linux rate-classification keeps working unchanged. Don't
  add a second health path.
- **`wasapi.rs`'s format ladder and period math live above the `cfg`**, plain Rust
  with their own tests, so they compile and run on Linux CI. rox's reasoning, and it
  applies harder here since nobody develops this tree on Windows.
- **`src/ui/` reaches the output layer through `library::playback::*`**, never
  directly — the repo-wide rule, and the device picker is the obvious place to break it.
- **`player/output/` owns no persistence.** Settings go through `mutate_settings` +
  the kick-after-persist rule like every other playback flag.

---

## Phases

Each phase is independently shippable and leaves the tree working. Platform phases
are independent of each other — the seam falls back to shared wherever a backend
doesn't exist.

### Phase 1 — The output seam · **mostly done by #90**

`src/player/output/` exists, `AudioOutput` is owned on `AppState`, `device::open` carries
the fallback ladder and the `stream_health` callback, and `mixer::fill` is the single
point samples reach a device buffer — the ownership rule this doc wrote for `pump()`,
adopted there. What is left of this phase is the exclusive-mode vocabulary, which had
nowhere to attach before there was a seam:

1. Widen `Negotiated` (currently `{ shape, format }`) with `mode`, `device`, `engaged`
   and `reason`, and add `OutputMode { Shared, Exclusive }` plus
   `OutputRequest { mode, device, rate, channels, format, period_ms }`.
2. Put today's `device.rs` behind a `trait OutputBackend` so a platform backend is a
   sibling file rather than an arm in every match. `device::open`'s `build` closure
   already inverts the construction the right way for this.

**Exit:** `cargo clippy --all-targets --locked -- -D warnings` clean, `cargo test`
clean, playback behaves identically. No user-visible change.

### Phase 2 — Source format on the playback projection

1. Add `sample_rate`, `channels`, `bit_depth` to `TrackSummary` (`Option<i32>`,
   `#[serde(default)]` — the queue round-trip persists it).
2. Carry them through the projection queries in `src/database/queries/track.rs` and
   whatever else builds a `TrackSummary`. Follow the ReplayGain columns exactly.
3. Surface the negotiated-vs-source pair in the UI as read-only text (Now Playing
   detail, or Settings → Playback). It reads "FLAC 44.1 kHz / 24-bit → device 48 kHz"
   today, which is useful on its own and proves the data path before anything acts
   on it.

**Exit:** the rate shown for a playing track matches `ffprobe`. No playback change.

### Phase 3 — Reopen at rate · the structural spine

The risky phase. Built against the **shared** backend so a bug is a glitch.

1. `PlaybackEngine::reopen_at(rate, channels)`: under the decks lock, drop both decks,
   drop the backend, build a fresh `mixer::pair(DECK_COUNT, shape)`, open the backend
   against it, rebuild `Decks`. `EqShared` / `ReplayGainShared` / `VisualizerShared` are
   rate-independent `Arc`s and survive; `FadeShared` is deck-scoped and is rebuilt
   with the decks.
2. Wire it as a side effect on the `emit_and_execute` path — it must run under
   `exec_lock`, never with the `PlayerState` lock held.
3. Two entry points: **at a track boundary** when the next track's rate differs (the
   common case; the reopen lands in the gap), and **mid-track** when the user toggles
   the setting, which re-plays the current track at its position.
4. Gate the whole thing behind a new `follow_source_rate` flag, default **off**. On
   its own this is already worth shipping on Linux: PipeWire honours a requested rate
   in `default.clock.allowed-rates`, so shared mode plus rate-following removes one
   resample without any FFI.
5. Suppress gapless preload and crossfade across a rate boundary while the flag is
   on — two tracks at different rates cannot both be bit-perfect, and a fade would
   need one of them resampled. Reuse the existing `crossfade_eligible` predicate;
   add the rate term there rather than a second gate.

**Exit:** toggling the flag and playing a 44.1 / 48 / 96 kHz sequence reopens cleanly
each time, position and queue survive, no deadlock under `--test-threads=1` or
parallel. Peak RSS unchanged (measure with `/usr/bin/time -v` on release once, at the
end of the phase).

### Phase 4 — The bit-perfect contract · settings + the truth panel

1. `BitPerfectFlags` in `src/services/settings/data.rs` — `#[serde(default)]`,
   `#[serde(flatten)]`'d like `PlaybackFlags`. Fields: `enabled`, `mode`
   (`shared` / `exclusive`), `device_id`, `period_ms`, `resync_delay_ms`.
   Ships **off**, per the new-visible-behaviour default.
2. The bypass matrix, enforced in one place: with bit-perfect on, EQ / ReplayGain /
   crossfade are forced off and their controls disabled with a reason, speed pins to
   1.0, and the volume slider disables at 100%. Do this by *disabling the controls*,
   not by silently ignoring them — a slider that moves and does nothing is worse than
   one that won't move.
3. The truth panel in Settings → Playback: mode, device, negotiated rate / format,
   and either **Engaged** or the single reason it isn't, taken from
   `Negotiated::reason`. This is the deliverable that makes the claim checkable.
4. **The round-trip test.** Fixtures: a short 16-bit and a short 24-bit WAV. Build
   the full chain with everything bypassed, pull `MixerPull` directly (via
   `output::mixer::pair`, as `tests/crossfade.rs` already does), assert the
   samples are bit-identical to the decoder's output. No device needed. This is the
   test rox doesn't have, and it's what stops a future DSP change quietly breaking
   the claim.

**Exit:** the panel tells the truth in every combination; the round-trip test passes
and fails if you re-enable any node. New strings have a `msgid` in all six catalogs.

### Phase 5 — Linux exclusive · ALSA `hw:` + device reservation

1. `output/reserve.rs`: acquire `org.freedesktop.ReserveDevice1` for
   `org.freedesktop.ReserveDevice1.Audio<N>` before the claim, release on drop.
   **`zbus::blocking::Connection` inside `spawn_blocking`** — never `features =
   ["tokio"]` on zbus, which panics Slint's a11y thread (`CLAUDE.md`, Known Gaps).
   Failing to reserve is a fallback reason, not an error.
2. `output/alsa.rs`: enumerate via card/pcm info rather than name hints (hints are
   full of `default` and plugin aliases, none of which is a claim on hardware); open
   `hw:CARD=x,DEV=n`; `set_rate_resample(false)`; negotiate format via the shared
   ladder; period and buffer from the request with the tree's usual named constants.
3. Writer thread: blocking `writei`, `pump()` per period, `try_recover` on xrun with
   a bounded retry, report through `stream_health`. Ask for `SCHED_FIFO` best-effort
   and degrade quietly when `RLIMIT_RTPRIO` says no — most desktops allow it via
   `rtkit`, and a failure to get it must not fail the claim.
4. Note in the module docs that `PIPEWIRE_ALSA` (set in `main.rs`) does nothing in
   this mode: the `hw:` claim bypasses pipewire-alsa, so the stream stops appearing
   in `pavucontrol` entirely. That's the point, and it's the first thing a user will
   report as a bug.

**Exit:** exclusive engages on a real card; a second app's audio is silenced;
`/proc/asound/card*/pcm*p/sub*/hw_params` shows the file's rate and format; toggling
off releases the card and other apps recover. Busy device falls back with the right
reason.

### Phase 6 — Windows exclusive · WASAPI event-driven

1. `windows = "0.62.2"` pinned to match cpal (finding 4), feature list limited to the
   modules the backend names.
2. Format ladder, `period_hns`, `buffer_hns`, `aligned_hns` as plain Rust above the
   `cfg`, with unit tests that run on Linux.
3. COM on a thread we own (the UI thread is an STA; an interface created there can't
   legally be touched from the writer). `IsFormatSupported == S_OK` exactly. The
   alignment dance with a **freshly activated** client on retry.
4. **Event-driven**: `AUDCLNT_STREAMFLAGS_EVENTCALLBACK`, `SetEventHandle`,
   `hnsPeriodicity == hnsBufferDuration`, writer blocks on `WaitForSingleObject`.
   `AvSetMmThreadCharacteristics("Pro Audio")` on that thread, released on exit.
5. Staging buffer + one `copy_nonoverlapping` per period — nothing promises the
   driver's pointer is aligned for the sample type.

**Exit:** engages on a real endpoint; the Windows volume mixer shows Melodia gone
while it plays; a busy endpoint falls back with a reason.

### Phase 7 — macOS exclusive · hog mode

1. `objc2-core-audio` pinned to `0.3.2`. No hand-declared `extern "C"`, no
   four-char-code constant table — that's the whole reason for the dependency.
2. Take hog mode (guard the toggle-on-set behaviour; the read-back decides), set the
   nominal rate and poll until it settles, read back the buffer frame size, attach an
   IOProc, watch `kAudioDevicePropertyDeviceIsAlive`.
3. Release hog mode **last** in `Drop`, after the IOProc is destroyed — releasing
   earlier lets another app reconfigure the rate under a live IOProc.
4. Handle both buffer layouts: one interleaved buffer (every built-in output and most
   USB DACs) and one buffer per stream (pro interfaces, aggregate devices).

**Exit:** engages on a real device; other apps go silent; unplugging trips the reopen
path; toggling off restores system audio.

### Phase 8 — Polish

1. Device picker in Settings → Playback, populated per mode (the ids don't cross —
   a cpal device name means nothing to the ALSA backend, so ask for the two lists
   separately).
2. Period / buffer knob and the resync delay, both with the device's own limits as
   bounds and both reported back as negotiated.
3. Packaging: `alsa-lib-devel` / `libasound2-dev` build deps on the RPM and DEB specs
   and the AUR PKGBUILD; check the CI Linux runner has them. Read
   `.claude/rules/ci-packaging.md` before touching any of it.
4. `.claude/rules/unsafe-rust.md`: the sanctioned-site table currently lists ten calls
   in five files, and this feature adds a much larger FFI surface. Decide and write
   down which — extend the table, or give each `output/<platform>.rs` a module-level
   `#[allow(unsafe_code, reason = "…")]` under the rule's "unless every item in it is
   FFI" clause. Don't leave the rule silently out of date.
5. `README.md` feature list, `CLAUDE.md` conventions, and a new
   `.claude/rules/bit-perfect.md` scoped to `src/player/output/**` holding the
   contract and the per-platform quirks — it cuts across enough that a nested
   `CLAUDE.md` would miss the settings and UI halves.

---

## Cross-cutting

- **Memory.** One writer thread and one staging buffer sized to the device period per
  claim; the reopen drops the old mixer and decks before building the new ones. No new
  caches. Measure peak RSS once at the end of Phase 3 and once at the end of Phase 5;
  anything over the usual ceiling gets `heaptrack` before it merges.
- **Threading.** Backends run on their own threads and never touch Slint. The reopen
  is a side effect on the `emit_and_execute` path — `exec_lock → PlayerState → decks`,
  never reversed.
- **Errors.** `AppError` boundary variants with `#[source]`, and every fallback reason
  is a user-facing string that names what failed, not that something did.
- **i18n.** Every new literal needs the same `msgid` in all six `.po` catalogs; the
  device names and negotiated formats are data and stay untranslated.
- **Logging.** Nothing in `pump()` or a writer loop logs — the health counters exist
  for exactly this. The last thing a dying writer thread does may log.

## Open questions

- Does WirePlumber release the card promptly enough on reservation, or does it need a
  settle delay of its own? Decide against real hardware in Phase 5.
- Whether the truth panel belongs in Settings → Playback or on Now Playing. Settings
  for v1; revisit if users can't find it.
- Whether `follow_source_rate` (Phase 3) is worth exposing on its own as a shared-mode
  setting, or should only ever appear as part of the bit-perfect toggle.
