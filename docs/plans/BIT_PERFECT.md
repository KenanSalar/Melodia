# Bit-Perfect Output

Working doc. Delete it when the feature ships. Tracks issue #66.

Status: **Phase 1 done** (2026-09-24) · **Phase 2 done** (2026-09-25) · **Phase 3 done** (2026-09-25) · **Phase 4 done** (2026-09-27) · **Phase 5 done** (2026-09-27) · Phase 7 in progress: Windows steps 1 to 6 done (step 6 is hardware volume, Gate B's volume half passed), 24-bit and 192 kHz WASAPI output verified, the Linux half done (2026-09-28: hardware volume on ALSA, the lead, the period bound, the card's own level in the panel), the rest sorted by platform under "What's left of Phase 7" · macOS moved to #113 · Rewritten: 2026-09-23 (replaces the 2026-08-14 draft)

## What the user sees today

Every track goes through the OS mixer at whatever rate the system output happens to run at,
usually 48 kHz. A 44.1 kHz CD rip gets resampled on Linux and Windows. Nothing tells the user it
happened. They also cannot find out what Melodia hands to the DAC, because the negotiated format
only goes to the log. When a USB DAC is unplugged, the user gets a toast and silence until
Melodia restarts.

## What ships

1. **Match the file rate**: the output reopens at each track's sample rate. In shared mode this
   helps wherever the OS lets a stream set the rate.
2. **Exclusive output**: Melodia takes the device for itself, runs it at the file's rate and
   format, and hands it the decoder's samples.
3. **A signal path panel** that tells the truth. It shows each stage from file to DAC, gives a
   verdict (**Bit-perfect**, **Enhanced**, **Converted** or **Fallback**), and names whatever
   is keeping playback from being bit-perfect. A **Make bit-perfect** button resets those things.
4. **Recovery from device loss**. This comes free with the reopen machinery and ships first.

The claim the panel makes, and which a test pins:

> When the panel reads **Bit-perfect**, the device receives the decoder's samples bit for bit.

Here is what breaks it. The panel names each of these rather than hiding it:

| stage | breaks bit-perfect when |
|---|---|
| output | not exclusive (the OS mixer may convert or mix), or fell back from exclusive |
| rate | device rate != source rate (`output::convert` interpolates) |
| channels | a downmix or a wider layout. Mono duplicated to stereo at unity is exact and does **not** break it |
| source | a 32-bit integer source. f32 carries 24 bits, so its low 8 bits are lost |
| DSP | EQ active, ReplayGain gain other than unity, the limiter engaged (all inside `EqSource`, whose bypass is already exact) |
| speed | anything but 1.0 |
| volume | anything but 100 %, or muted (`voice.rs` skips the multiply only at bitwise unity) |
| crossfade | a fade in progress (and crossfade is ineligible while the output follows the rate, see Phase 2) |

The visualizer tap stays on. It is read-only and sits above the deck's conversion and volume.

**Not in v1:** DSD/DoP, ASIO, upsampling, dither, a picker for the shared-mode device, and a
native PipeWire exclusive stream (see Open questions).

## What changed since the 2026-08-14 draft

Checked against the tree at `46ad594a` and against cpal 0.18.2:

- **rodio is gone (ADR 0007), which spends the old Phase 1.** `output/` exists and
  `AudioOutput` is owned on `AppState` (`crates/melodia-app/src/state/mod.rs:56`).
  `MixerPull::fill` (`output/mixer.rs:70`) is the only fill point, and it keeps `-0.0` intact.
  Equal-rate conversion is exact and pinned (`convert_tests.rs:28,39`). The unity skip is at
  `voice.rs:362`. `Negotiated` already carries `shape`, `format`, `requested_period` and
  `period` (`device.rs:49`). Issue #66's body still blames rodio and is out of date.
- **None of the exclusive half exists.** There is no `OutputMode`, no device setting, no
  reopen, and no bit-depth plumbing: the decoder throws `bits_per_sample` away in
  `decode.rs` `fill`.
- **cpal 0.18.2 has no exclusive mode on any host.** The WASAPI exclusive PR (RustAudio/cpal#1368)
  is parked until 0.19's backend-extension traits land (#1220). The PipeWire host exists behind a
  feature flag, but only master sets `node.rate` (#1341, unreleased). So Melodia owns its
  exclusive backends. They sit behind one seam, so a later cpal can replace them one at a time.
- **The old spine ("rebuild the decks against a new mixer") is replaced by reshaping in place.**
  See Phase 1. The decks, their fade cells, a staged gapless source and the position all survive
  a reopen, because only the stream is replaced.
- **Wrapper crates keep the new `unsafe` count at zero.** `alsa` 0.11 is safe. The `wasapi`
  crate wraps COM. A raw `windows` backend would add dozens of sites against
  `unsafe_code = "deny"`.
- **No ring buffer.** A backend's writer thread owns `MixerPull` and calls `fill` directly. The
  old draft's per-sample ring-drain critique is moot.
- **Packaging is unaffected.** `libasound` is already linked through cpal.

## How the best implementations do it (mechanisms, no attribution)

Surveyed: the sibling checkouts under `~/Development`, plus current docs and changelogs.
- Exclusive claims: ALSA `hw:` with resampling off. WASAPI exclusive, **event-driven**,
  `buffer == period`, integer PCM tried before float (many drivers refuse float in exclusive),
  `IsFormatSupported == S_OK` exactly, a fresh client on `BUFFER_SIZE_NOT_ALIGNED`, MMCSS on
  the writer thread.
- **The reopen is decided before the track plays**, from the decoder's format. Gapless holds
  within a format, and the gap falls at a format boundary. The weaker design notices the
  mismatch after the track is already audible and rebuilds the whole session.
- **Format ladders start from the source depth.** One implementation left out packed 24-bit
  and so falls back on DACs that only offer `S24_3LE`, which is common for USB. **`S24_LE`
  is LSB-aligned in 32 bits, and WASAPI's 24-in-32 is MSB-aligned.** Mixing the two up is the
  48 dB-too-quiet bug cpal fixed in 0.18.2 (#1309).
- **On Linux, nothing in the survey coordinates with the sound server.** The fix is
  `org.freedesktop.ReserveDevice1`. The session manager reserves each card at priority −20,
  and a normal app at priority 0 outranks it and receives the card. An owner must also answer
  `RequestRelease`.
- **Resync delay.** DACs mute while their clock relocks after a rate change, which clips the
  start of the track. The fix is a short, configurable run of silence after each reopen.
- **UX:** off by default. Each stage is graded with a colour and a reason. When the claim
  fails, the player falls back to shared visibly and never goes silent. On Windows the reason
  names the "Allow applications to take exclusive control" checkbox when that is the cause.
- **USB drivers that stutter in event mode** are real. A polling-exclusive compatibility
  switch is the known fix.

## Structure

```
crates/melodia-playback/src/player/playback/output/
  mod.rs        AudioOutput: open / reopen / close. OutputRequest, OutputMode, Negotiated (moved up)
  device.rs     the cpal backend (shared). Ladder prefers the requested shape
  mixer.rs      + MixerPull::reshape, MixerPull::hold (resync silence)
  voice.rs      + VoicePull::reshape, Voice::playing (what the signal path reads off a deck)
  convert.rs    unchanged
  encode.rs     NEW: DeviceFormat + the ladder + the one f32 -> device-bytes conversion for owned backends
  claim.rs      NEW: ClaimError (typed fallback reasons, a local std::error::Error)
  alsa.rs       NEW  #[cfg(target_os = "linux")]
  reserve.rs    NEW  #[cfg(target_os = "linux")]   ReserveDevice1 over zbus::blocking
  wasapi.rs     NEW  #[cfg(target_os = "windows")]
crates/melodia-engine/src/player/engine/
  signal_path.rs NEW: pure evaluate(inputs) -> SignalPath { stages, verdict }
```

Ownership rules:
- **`MixerPull::fill` stays the only place samples are produced for a device.** For owned
  backends, `encode.rs` is the only place they become device bytes. The cpal path keeps its
  typed `FromSample`, where cpal owns the byte layout.
- **`PlaybackEngine` owns `AudioOutput`**, which moves off `AppState`. The reopen has to run
  under the decks lock, and the engine is what holds it. Lock order becomes
  `exec_lock → PlayerState → decks → output`, never reversed.
- **`signal_path::evaluate` is the only place the verdict is computed.** It is pure, and the
  UI reads it through a `watch`. That is the "one place" the old bypass matrix wanted, without
  locking any controls.
- `stream_health` stays the one health path. Every backend reports through it.
- `melodia-views` reaches all of this through `library::playback::*`.
- Settings persist through `mutate_settings` + kick-after-persist. `output/` persists nothing.
- New thread names fit 15 bytes (`alsa-out`, `wasapi-out`). Files stay under 800 lines.
- `libc` stays confined to `allocator`. Anything that needs a tid or an rlimit goes through
  whatever Phase 4's entry check picks.

## Phases

Each phase leaves the tree shippable. Phases 4 and 5 are independent: the seam falls back to
shared wherever a backend doesn't exist. **Per phase: implement → static gates (fmt, clippy
`--workspace`) → Kenan tests by hand → then tests and docs** (memory: manual test gates tests
and docs).

### Phase 1: Reopen in place, and recover from device loss ✅ done

The spine. It is built and shipped against the shared cpal backend, where a bug costs a glitch
rather than a dead card.

1. **The stream hands `MixerPull` back.** `device::open` takes the `MixerPull` by value. The
   cpal callback holds it in an `Arc<parking_lot::Mutex<_>>` and `try_lock`s it: uncontended
   except at close, and it writes silence if the lock is held. `DeviceStream::close()` pauses,
   drops the stream and `Arc::try_unwrap`s the puller back out.
2. **`MixerPull::reshape(device: Shape)`.** Each `VoicePull` first services its pending
   commands, so an append built against the old shape is mounted rather than lost. It then
   rebuilds every loaded source's `Converter` (the playing one and a staged one) against the
   new shape, and resizes `scratch`. `Voice`'s control-side `device` becomes a shared cell, so
   later appends build against the new shape. Nothing else changes: the decks, the fade cells,
   the positions and the visualizer slots are all untouched.
3. **`AudioOutput::reopen(request) -> Result<Negotiated, AppError>`**: close → reshape →
   open. If the open fails, it reopens the previous request. If that fails too, the puller
   stays parked and the error reports device loss. The whole thing runs under the decks lock,
   on the `emit_and_execute` path, and never on the UI thread.
4. **`OutputRequest { mode, device, shape: Option<Shape> }`** and
   **`OutputMode { Shared, Exclusive }`**. `Negotiated` moves to `mod.rs` and gains `mode`,
   `device_name` and `fallback: Option<ClaimError>`. `claim.rs` defines `ClaimError`. The
   variants cover busy, not allowed, reserved by another app, format refused, rate refused,
   device gone, and I/O with a typed `#[source]`. It is never a `String`.
5. **Device-loss recovery**, as the first consumer. On `device_lost`, `tasks::audio_health`
   asks the engine to reopen the current request, with a bounded backoff. If the reopen fails,
   it keeps today's toast. Keep `stream_health`'s three-way split: xruns and non-fatal kinds
   never trigger a reopen.
6. Publish `Negotiated` on a `watch` from the engine. The boot log line reads from it.

**As built (2026-09-24), where it departs from the steps above:**
- **The device shape left `Converter`.** It allocated only by source width and its interpolation
  state lives on the source's timeline, so `fill` now takes the device and nothing loaded depends
  on it. Reshape is a field assignment plus a `scratch` resize: no converter rebuild, no shared
  device cell on `Voice`, and no append/reshape race to guard.
- **The puller never leaves `AudioOutput`.** It sits in an `Arc<Mutex<MixerPull>>` every stream
  clones, so dropping the stream is the whole handoff and nothing rests on when cpal drops its
  callback. The ladder reshapes it per rung instead of building a mixer per rung.
- `take_device_lost` on `stream_health`, polled every 250 ms by `tasks::audio_health`, so a
  loss is not held for a 5 s drain window. The backoff is `REOPEN_BACKOFF` there.
- **A stall is a loss too.** Restarting `PipeWire` under the ALSA plugin reports nothing: no
  `POLLHUP`, and cpal polls with no timeout, so the data callback just stops. The callback beats
  `AudioStreamHealth::blocks` on every block (silence included), and `StallWatch` reopens after a
  second without movement. Found in the first manual test, where recovery never fired.
- **Deferred to the phase that first reads them:** `OutputRequest`, `OutputMode`, `ClaimError`
  and the fall-back-to-the-previous-request arm (Phases 2 to 4: in Phase 1 every request is the
  default device), and the `Negotiated` watch plus its move to `mod.rs` (Phase 3's panel).
  Phase 1 exposes `PlaybackEngine::negotiated()` and adds `device_name` to `Negotiated`.

**Exit:** unplugging the DAC mid-track continues playback on the new default output at the same
position. A crossfade or a staged gapless track in flight survives a forced reopen. No deadlock
under parallel tests. Measure peak RSS once (`/usr/bin/time -v`, release).
**Tests after the go:** a device-free reshape test that pulls `mixer::pair` across a reshape
and asserts position and sample continuity, plus a reopen-failure test that falls back to the
previous request.

**Result.** A `PipeWire` restart mid-track resumes at the same position after about a second
(manual, 2026-09-24). The tests landed are:
- `mixer_tests`: a reshape continues from the same source frame, and a wider reshape still sums
  two voices.
- `crossfade.rs`: a crossfade and a staged gapless handover each survive a mid-transition
  reshape, both confirmed to fail against a reshape that skips the voices.
- `stream_health_tests`: `take_device_lost` and the heartbeat.
- `audio_health_tests`: `StallWatch`.

The reopen-failure fallback test moves to Phase 2 with the fallback itself. The no-device toast
and the release RSS reading were not run.

### Phase 2: Source format truth and following the file rate ✅ done

1. **`AudioSource::format() -> SourceFormat { bits: Option<u8>, float: bool }`**, read from
   Symphonia's `AudioCodecParameters` (`sample_format`, `bits_per_sample`) in `decode::open`.
   It is carried on `decode::Opened` / `Cursor` and on both decoders. It is the source of
   truth for the panel and the ladder. The database's `bit_depth` is not used for either.
2. **The ladder prefers the requested shape.** Given `request.shape`, `device::ladder` puts
   the rungs whose range contains that rate and channel count first. The chosen rung shows up
   in `Negotiated`.
3. **The reopen happens at the boundary, decided from the decoder:**
   - `play_media` compares `decoded.shape()` with the negotiated shape. If rate-following is
     on and they differ, it reopens before the append.
   - `preload_gapless` makes the same comparison. On a mismatch it refuses to stage, so the
     transition goes through `EndOfStream → PlayMedia` and reopens there. A per-path refusal
     latch stops the monitor from reopening the file on every 500 ms tick.
   - `crossfade_eligible` gains a `follows_rate` term. **Crossfade is off while the output
     follows the rate**, because a fade cannot cross a reopen. The UI says so on the crossfade
     row. Adding this term means no `TrackSummary` change is needed.
4. **Resync delay.** After a reopen, `MixerPull::hold(frames)` emits silence first. The default
   is a named constant, tuned against hardware in Phase 4.
5. **Settings.** `OutputFlags` goes in `crates/melodia-app/src/services/settings/playback.rs`
   with `#[serde(default)]` and is flattened into `SettingsData`. Fields: `mode` (a persisted
   key, not an index), `follow_rate`, `device`, `resync_delay_ms`. Everything defaults off or
   shared. The Settings → Playback **Output** card gets a "Match the file's sample rate" toggle.
   It is hidden on Windows, where shared mode's `AUTOCONVERTPCM` makes it a no-op.
6. **Entry check (Linux):** with `default.clock.allowed-rates` set, confirm with `pw-top` or
   `hw_params` whether opening cpal's ALSA default at 44.1 kHz moves the PipeWire graph. If it
   doesn't, the Linux half of this toggle waits for a cpal release that carries #1341, and the
   panel reports "Converted by system mixer". Record the answer here.

**Exit:** a 44.1 / 48 / 96 kHz sequence reopens at each boundary. A same-rate album stays
gapless. On Linux, the entry check's answer holds.

**As built (2026-09-24), where it departs from the steps above:**
- **`SourceFormat` moved to Phase 3**, its first reader. Nothing here reads bit depth.
- **Only the rate is followed.** `OutputRequest { rate }` keeps the device's channel count, so a
  mono or 5.1 file never hands the OS a remix. `mode` and `device` join it in Phases 3 and 4.
- **`OutputFlags` is `output_follow_rate` alone**, and the resync is `output::RESYNC_HOLD`
  until Phase 7's knob. The hold services voice commands while it plays silence. Otherwise the
  `clear` that `cut_to` issues right after the reopen would wait out `SERVICE_TIMEOUT`.
- **Crossfade is masked in `PlaybackEngine::crossfade_settings`**, not by a new term in
  `crossfade_eligible`. That covers the manual fade too, which can't cross a reopen either.
- The reopen-to-previous-request fallback landed in `AudioOutput::reopen`, and the device-loss
  path now reopens the current request rather than the default.
- The toggle acts from the next track, in both directions.
- Engine half in `backend/output.rs`. The Settings card is `output-section.slint`, hidden on
  Windows.
- **Entry check, answered (2026-09-25): yes, where PipeWire allows the rate.** By default
  `clock.allowed-rates = [ 48000 ]` and nothing moves. With
  `pw-metadata -n settings 0 clock.allowed-rates "[ 44100 48000 88200 96000 ]"`, opening cpal's
  ALSA default at the file's rate moves the graph: 44.1 → 96 → 48 kHz each showed in `pw-top`
  and in the card's `hw_params`. Three limits hold:
  - **A rate the card lacks gets the nearest one**, resampled by PipeWire: 88.2 kHz ran the
    hardware at 96 kHz. Phase 3's panel can't see that from `Negotiated`, which reports the
    PipeWire stream, not the card.
  - **Another app's running stream pins the graph.** Melodia's reopen drops its own stream
    first, so it is only ever other apps that hold it. An `aplay` at 44.1 kHz stayed resampled
    while Melodia's always-on stream ran.
  - **The pin outlives the app that set it.** `PipeWire` settles the card's rate when a stream
    starts, not when one leaves, and Melodia's stream never idles, so the card stayed at 48 kHz
    after the browser holding it closed. So while following, every fresh track start reopens, same rate
    included. That costs no resync silence, because the hold fires only on a rate change.
    Gapless transitions don't reopen, so an album started under the pin stays resampled until
    its next non-gapless start.

**Result.** Manual tests on 2026-09-25, reading `pw-top`, the card's `hw_params` and the log:
- **Following the rate:** a 44.1 / 96 / 48 kHz sequence moved the card at each boundary.
- **Gapless:** a 44.1 → 44.1 → 48 kHz queue reopened only at the rate change, so the
  same-rate transition stayed gapless.
- **Taking the card back:** once the browser pinning the card at 44.1 kHz closed, a
  same-rate start moved it to 96 kHz.
- **Log:** no warnings across the runs.

The tests landed are:
- `mixer_tests`: the hold plays silence before the source's first frame, the clock stands
  still through it, and a clear issued during it still lands.
- `device_tests`: the requested rate leads the ladder, and the fallback walk is unchanged.
- `backend_tests`: following the rate masks the track-change fades but keeps fade-on-pause,
  and crossfade comes back when following stops.

Each pinning test was confirmed to fail against a mutation of the code it covers. Two paths need
a real device and have no unit test: the refused gapless stage, and the fallback to the previous
request. The queue run above exercised the first.

### Phase 3: The signal path panel and "Make bit-perfect" ✅ done

1. **`signal_path::evaluate`** is pure. Its inputs:
   - `Negotiated`
   - the playing source's `SourceFormat` and shape
   - EQ and ReplayGain state, read from `EqSource`'s bypass condition and not recomputed
   - speed, volume and mute
   - whether a crossfade is in flight

   It returns stages plus a verdict:
   - **Bit-perfect** requires exclusive output and every row in the table above clean.
   - **Enhanced** means only DSP the user chose (EQ, RG, volume, speed) touched the samples.
   - **Converted** covers a resample, a downmix, or shared output.
   - **Fallback** means exclusive was asked for and refused. The reason is shown.

   **Shared output never reads Bit-perfect.** The OS mixer is out of our sight.
2. It is published on a `watch`, re-evaluated whenever an input changes, and never evaluated
   per sample.
3. **UI:** in Settings → Playback **Output**, the mode picker (Shared / Exclusive, exclusive
   greyed until the platform's backend exists), the stage list, and the verdict.
   **Make bit-perfect** calls one `library::playback` function that turns EQ and ReplayGain
   off, sets speed to 1.0 and volume to 100 through the existing setters, so each persists as
   usual.
4. All new strings get a msgid in all six catalogs.

**Exit:** the panel is right across the toggle matrix: EQ, RG, speed, volume and mute, each
on and off, over shared, exclusive and fallback.
**Tests after the go:** a table-driven unit test over `evaluate`.

**As built (2026-09-25), where it departs from the steps above:**
- **The mode picker, `OutputMode`, `ClaimError` and the Fallback verdict moved to Phase 4**, their
  first reader. With no exclusive backend the picker would have one live option and the verdict an
  arm nothing reaches. So until Phase 4 the headline reads Converted whenever something plays, and
  the stage rows carry the detail.
- **`SourceFormat` is `{ bits: u8, float: bool }`, and `bits` is always known.** It comes from the
  first decoded buffer's sample type, narrowed to the track's stated `bits_per_sample`. The
  narrowing is not optional: Symphonia decodes every FLAC, 16-bit included, into 32-bit samples,
  so without it every FLAC would read as losing bits. Carried on `decode::Opened` and returned by
  a new required `AudioSource::format`; `PrebufferSource::new` takes the stream decoder's answer.
- **The DSP stage reads `EqSource`'s own bypass** through a second required method,
  `AudioSource::dsp_engaged`. An EQ that is on with nothing to do at this rate stays clean. The
  fade is left out of it, because a crossfade is its own stage.
- **The voice reports what it is playing.** `Voice::playing()` returns shape, format and
  `dsp_engaged`, stored with the clock at takeover and once per render. A gapless handover
  updates it without the engine having to know when the handover happened.
- **One `watch` carries the whole `SignalPath`**, not a `Negotiated` watch beside it
  (`AppState::signal_path_tx`). The playback monitor evaluates it on its 500 ms tick while
  playing, with `send_if_modified`, and publishes `None` on Stop. A pause keeps the last path,
  since nothing pulls the source to re-read it. `Negotiated` moved to `output/mod.rs` and gained
  `PartialEq`.
- **Make bit-perfect also turns on following the file's rate** where that does anything
  (`library::playback::FOLLOW_RATE_SUPPORTED`, now the one home of the Windows answer). Everything
  it resets is persisted in one `mutate_settings`.
- UI: `globals/signal-path.slint` (`SignalPathUi`), eight stage rows under a verdict row in
  `output-section.slint`, wired by `ui/settings/signal_path.rs`.

**Result.** Manual toggle matrix over shared output (2026-09-25): each row followed its setting.
A forwarded 44.1 / 48 / 88.2 / 96 kHz queue reopened at every boundary, with no warnings in the
log. One limit shows in that run:
- **The panel cannot see past PipeWire.** At 88.2 kHz the card ran at 96 kHz while `Negotiated`
  reported the 88.2 kHz stream, so the Sample Rate row read clean. Only the Device row ("shared,
  the system mixer may still convert it") and the Converted headline stay honest there. Exclusive
  output removes the gap, since the stream is then the card.

The tests landed are:
- `file_decode_tests`: the format each fixture decodes to, including the new
  `silence-24bit.flac` and `silence-32bit.wav`.
- `equalizer_tests`: `dsp_engaged` against the bypass (EQ off, on and flat, an active band, a
  band past Nyquist, ReplayGain untagged and tagged) and after a live change.
- `voice_tests`: nothing is reported before a mount, and the report follows a gapless handover.
- `signal_path_tests`: shared never reads Bit-perfect, and one row per stage boundary.

Each was confirmed to fail against a mutation of the code it covers.

### Phase 4: Linux exclusive: ALSA `hw:`, ReserveDevice1, device picker ✅ done

1. **`encode.rs`** is plain Rust with no `cfg`.
   - `DeviceFormat { S16, S24Packed /*S24_3LE*/, S24Low /*S24_LE*/, S32, F32 }`.
   - A ladder that starts from the source depth:
     - 16-bit: S16, S32, S24Packed, S24Low
     - 24-bit: S24Packed, S32, S24Low
     - float or 32-bit: S32, F32
   - Each rung fed by power-of-two scaling, saturating at +1.0.
2. **`alsa.rs`:**
   - Enumerate with `card::Iter` + `Ctl::pcm_info`, which gives stable
     `hw:CARD=<id>,DEV=<n>` ids and card names. Never use name hints.
   - Open `hw:` with `set_rate_resample(false)`, the **exact** rate (refused means fallback,
     never "near") and the exact channel count, then walk the format ladder.
   - Period and buffer come from named constants.
   - The writer thread `alsa-out` owns the `MixerPull`: `fill` → `encode` → blocking `writei`.
     `try_recover` retries a bounded number of times, then reports `DeviceNotAvailable`
     through `stream_health`, which goes into Phase 1's recovery.
3. **`reserve.rs`** runs over `zbus::blocking` on the session bus, inside `spawn_blocking`.
   Never enable the zbus `tokio` feature.
   - Acquire `org.freedesktop.ReserveDevice1.Audio<N>` at priority 0 before opening the card.
   - Serve `RequestRelease`: yield to a higher priority, which becomes the fallback reason
     "taken by <ApplicationName>".
   - Hold the claim through pause. Release it on stop, when the mode is turned off, and at quit.
   - A reservation that fails is a fallback reason, not an error.
4. **Real-time priority, best-effort and degrading quietly.**
   - **Entry check:** does `audio_thread_priority`'s Linux path pull in `libdbus`? That would
     be a new C dependency and a packaging change.
   - If it does, request RT through RealtimeKit over `zbus::blocking` instead.
   - Either way, failing to get RT never fails the claim.
5. **Device picker:** in the Output card, shown when the mode is Exclusive, listing ALSA cards.
   It persists the id. A missing device falls back with the reason "not connected".
6. Put in the module docs that the stream disappears from the sound server's mixer while
   exclusive, and that `PIPEWIRE_ALSA` has no effect here. Users will report both.
7. **Carried from Phase 3:** `OutputMode` on `OutputRequest` and a persisted `mode` key, the
   Shared / Exclusive picker in the Output card (exclusive greyed where no backend exists),
   `ClaimError`, and the Fallback verdict with its reason. `SignalInputs` then takes the mode
   rather than grading every output as shared, which is also the first time Bit-perfect and
   Enhanced can read.

**Exit:**
- `/proc/asound/card*/pcm*p/sub*/hw_params` shows the file's rate and format.
- Other apps go silent while exclusive holds the card, and recover when the mode is turned off.
- A card held by another client falls back with the right reason.
- The resync default is tuned on real hardware.

**Entry check, answered (2026-09-25): `audio_thread_priority` is out.** By default its Linux path
pulls the `dbus` crate, which brings `libdbus-sys`, a C library the Linux build doesn't link
today. It is also MPL-2.0. Its other path, a bare `SCHED_FIFO` call, needs `RLIMIT_RTPRIO` or
`CAP_SYS_NICE`, and desktop sessions have neither. So `output/realtime.rs` asks RealtimeKit over
`zbus::blocking`, and uses `rustix` (already in the lock) for the thread id and the
`RLIMIT_RTTIME` cap that rtkit requires.

**As built (2026-09-25), where it departs from the steps above:**
- **`OutputRequest` is an enum**: `Shared { rate }` or `Exclusive { device, shape, format }`.
  `Negotiated` gains `fallback` and a `format: OutputFormat` that says which backend opened it,
  so no separate `mode` field is needed.
- **`ClaimError` keeps the typed source for the log line. `FallbackReason` is what the panel
  reads**, because `Negotiated` has to stay `PartialEq` for the watch.
- **Exclusive output always follows the rate** and masks crossfade the same way. The follow-rate
  row is hidden under Exclusive, and the crossfade row greys out.
- **A same-format track start doesn't reopen a claimed card.** The Phase 2 reclaim is about the
  PipeWire graph, which `hw:` doesn't go through. A fallback is retried at every fresh start,
  because the card's holder may have let go.
- **A granted `RequestRelease` closes the card before it answers**, then reports a loss, so the
  usual recovery meets the new holder and falls back with its name.
- **With no session bus, the card opens unreserved**, since there is no server to coordinate
  with.
- **The mode picker is hidden where there is no backend** rather than greyed, matching the
  follow-rate row. `ChipGroup` has no per-option disable.
- **Make Bit-Perfect leaves the mode alone.** Taking the card from every other app is the user's
  call.
- Alsa is pinned at `0.11.0`, the version cpal resolves; `0.12` would be a second copy.

**Found in the manual test (2026-09-25 to 2026-09-27), all fixed:**
- **A hand-back strands the card in the session manager.** Taking a card and giving it back
  within milliseconds (every reopen, and every claim refused after the take) races the session
  manager's rebuild of the card. It either gave up on the card, which vanished from the system,
  or left it under a numbered node name. Desktop renames and the saved default output are keyed
  on that name, so both were lost until the session manager restarted. Three changes:
  - A reopen on the same card carries the reservation over (`alsa::Claim`).
  - A name taken from a holder goes back only once the holder has asked for it, plus a short
    grace so the refusal reaches it first (`reserve::Asked`).
  - The first open after a take retries while the card is busy (`alsa::BUSY_WAIT`): the
    session manager answers a release before it has closed the card.
- **zbus reports a held name as `Err(NameTaken)`**, not as `RequestNameReply::Exists`, so the
  first claim never asked the holder at all.
- **rtkit has no `Properties.GetAll`**, which zbus's default property cache calls, so every
  real-time request failed. Both proxies are built uncached.
- **A repeating queue with nothing playable skipped forever**, holding `exec_lock`, so no
  transport control could land. Not exclusive-specific, but found here: one start now skips at
  most a lap of the queue, then stops with a toast (`engine/actions.rs`).
- **The resync hold stays at 200 ms.** Neither test card mutes on a rate change (a beep train
  at 0 to 600 ms played whole at 0 ms of hold, on both). The hold is for the DACs that do, and it
  only falls at a rate boundary, which is never gapless.

**Result.** Manual and scripted tests on 2026-09-27 against an onboard ALC897 and a USB PCM2902,
reading `/proc/asound`, the reservation owner, `wpctl` and the session manager's log at info:
- `hw_params` shows the file's rate and format: S16 and S32 at 44.1 to 192 kHz, mono as one
  channel. Rates a card lacks fall back with the reason, as do 24-bit files on the 16-bit card.
- Rate changes on a held card reopen it without the session manager seeing anything, a
  same-format album stays gapless, and pause keeps the claim.
- Stop hands the card back under its own name, and the default output returns by itself.
- A card held by another client falls back as busy, and the next track start reclaims it.
- `RequestRelease` is refused at priority 0 and below, and granted above with the card
  already closed.
- Twelve rounds of refused claims, claims, rate changes and quick stop and play left no
  numbered names and no session manager errors.
- The writer runs `SCHED_RR`, granted by rtkit.

One thing the panel does not see yet: the card's own mixer. A card muted or attenuated there
plays silence or a quieter signal while the panel reads Bit-perfect. Reading the mixer on a
claim belongs with Phase 7's hardware volume. Answered in Phase 7's Linux half: the Volume row
names the card's level and mute, and the grade stays out of it.

The tests landed are:
- `encode_tests`: every integer layout hands the decoded extremes back, `S24_LE` is
  sign-extended low, full scale and beyond saturate, NaN is silence, float passes through, and
  `carries` over the ladder.
- `crates/melodia/tests/bit_perfect.rs`: 16-bit and 24-bit noise WAVs, written by the test,
  through the real engine and `encode` come out equal to the file's PCM, and an active EQ
  breaks the equality. No committed fixtures.
- `signal_path_tests`: over an exclusive claim, the headline for clean, a user choice, a
  conversion, a format too narrow for the source, and a fallback over each.
- `actions_tests`: a repeating queue of missing files stops after a lap, and a playable track
  behind missing ones still plays.

Each was confirmed to fail against a mutation of the code it covers. The reservation hand-back
has no unit test: it needs a real holder on a bus, and the scripted runs above are its coverage.

### Phase 5: Windows exclusive: WASAPI through the `wasapi` crate ✅ done

1. **Entry check:** `cargo search wasapi` and pin it. Confirm its `windows` dependency
   unifies with cpal's `0.62.2`. If it pulls a second copy, weigh the compile cost before
   adopting it.
2. The backend runs on a thread it owns (`wasapi-out`, COM initialised there) in
   `EventsExclusive` mode.
   - Format: `WAVEFORMATEXTENSIBLE` with integer PCM first, using `encode.rs`'s ladder. 24-bit
     goes in a 32-bit container with `wValidBitsPerSample = 24`, MSB-aligned. Keep that
     alignment separate from ALSA's `S24Low`.
   - Rate: exact.
   - Alignment retry through the crate's aligned-period helper, with a fresh client each time.
   - Pre-fill before `Start`, then one full buffer per event.
3. **Error mapping into `ClaimError`:**
   - `DEVICE_IN_USE` → busy.
   - `EXCLUSIVE_MODE_NOT_ALLOWED` → not allowed, with the reason naming the Sound-settings
     checkbox.
   - `DEVICE_INVALIDATED` → gone, which goes into Phase 1's recovery.
4. MMCSS through Phase 4's priority choice.
5. The device list comes from endpoint ids, shown in the same picker.

**Exit:** Melodia disappears from the Windows volume mixer while it plays. With the checkbox
cleared, it falls back with that reason. An unplug recovers.

**Entry check, answered (2026-09-27): adopted.** `wasapi` 0.24.0 (MIT) builds on `windows` 0.62,
the copy cpal 0.18.2 already resolves, so the lock gains the one crate and no second `windows`.
MIT puts nothing in `licenses/ATTRIBUTION.txt`.

**As built (2026-09-27), where it departs from the steps above:**
- **One seam instead of per-variant `cfg`s.** `output/mod.rs` picks a backend module per platform
  with one `cfg_select!`: `alsa`, `wasapi`, or the new `unsupported` everywhere else. Each answers
  the same names (`SUPPORTED`, `devices`, `open`, `ExclusiveStream`, `Claim`), so `Stream`, `claim`
  and `devices` lost every `cfg`, and `library::playback::EXCLUSIVE_SUPPORTED` reads
  `output::EXCLUSIVE_SUPPORTED` instead of spelling the platform list again. On Linux that was a
  rename, `AlsaStream` to `ExclusiveStream`.
- **Every COM call is made on the `wasapi-out` thread.** It opens the endpoint, answers the open
  over a channel, then feeds it, so no COM object crosses a thread and the caller's apartment
  doesn't matter. `Claim` is uninhabited there, since an endpoint has no reservation to carry.
- **`DeviceFormat::S24High` is the MSB-aligned 24-in-32**, encoded as the 24-bit value shifted up
  so it rounds at 24 bits and leaves the low byte clear. Both integer rows of the ladder carry it
  after `S24Low`, and each backend skips the rung it has no way to declare: ALSA `S24High`, WASAPI
  `S24Low`.
- **A plain `IsFormatSupported` comes before the crate's quirk walk.** The walk swallows every
  error, so a busy or barred device would read as one refusing the format. The first answer that
  refuses the whole device ends the claim. Phase 7's performance pass replaced the crate's walk with
  Melodia's own, which never offers the short header for integer PCM wider than 16 bits.
- **The Windows default endpoint is listed first**, so "no device chosen" and "the first one
  listed" stay the same device, as they are for a card on Linux.
- **The pre-fill is silence.** The resync hold is laid in after the open returns, and a block
  pulled before it would play ahead of the hold.
- **`NotAllowed` is a new `ClaimError` and `FallbackReason`**, the cleared checkbox. The panel's
  reason names the device's Advanced properties in the Sound control panel.
- **A quiet event wait (200 ms) asks the device whether it is still there**, since an endpoint that
  has gone can stop signalling rather than fail a call. Any error ends the writer and reports a loss.
- **MMCSS adds two `unsafe` calls**, missing the plan's "new `unsafe` at zero". No crate in the
  tree wraps `AvSetMmThreadCharacteristicsW`, and a normal-priority exclusive writer would sit below
  cpal's own shared thread, which runs at `THREAD_PRIORITY_TIME_CRITICAL`. Declarations from
  `windows-sys`, in `output/mmcss.rs`, listed in `unsafe-rust.md`.
- **Handing the card back moved into `AudioOutput::close_for`.** The shared arm of `start` had
  dropped the held claim explicitly, which on Windows drops an uninhabited type and fails clippy's
  `drop_non_drop`. Closing the old stream whole for a shared request does the same, on both
  platforms.
- No busy retry like `alsa::BUSY_WAIT`: nothing in the manual test needed one.

**Found in the manual test (2026-09-27):**
- **The refusal names the wrong device.** The log line and the panel's Device row name where the
  audio fell back to, not the device that refused, so two correct refusals on two devices read as
  one broken claim. And a lossy track is reported as a "32-bit source" with no "float". Both are
  open below.
- **Windows' Default Format has nothing to do with exclusive**; the two Exclusive Mode boxes do.
  They are readable, for a bug report, as the endpoint's registry properties
  `{b3f8fa53-0004-438e-9003-51a46e139bfc},3` and `,4` under
  `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\<id>\Properties`, `0` when
  cleared. With them cleared, `IsFormatSupported` and `Initialize` both answer
  `AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED` for every format and rate.

**Result.** Manual tests on 2026-09-27, Windows 11, reading the log and a query-only probe of each
endpoint:
- **Behringer UMC22** (a PCM2902, 16-bit at 32, 44.1 and 48 kHz only): a 48 kHz 16-bit file
  claimed it as `Exclusive(S16)` with a 960-frame (20 ms) period, and nothing else could play
  through it. Lossy tracks fell back with the format reason, correctly for a 16-bit-only device.
- **Realtek ALC897**: with the Exclusive Mode boxes cleared, every claim fell back as `NotAllowed`.
  Ticked, the same file claimed it as `Exclusive(S16)` at 48 kHz, a browser playing meanwhile was
  moved to another output, and it had the device back once Melodia closed.
- **Not run:** a rate change mid-queue, unplugging mid-track, Stop as distinct from closing,
  24-bit or high-rate output (no such files at hand), and the alignment retry. All but the last
  were run in Phase 7: the first three in Gate A, and 24-bit and high-rate output in its own run
  after Gate B.

The tests landed are:
- `encode_tests`: `S24High` puts the value in the top three bytes, rounds at 24 bits, and carries a
  24-bit source but not a 32-bit one.
- `wasapi_tests` (Windows only): what each rung is declared as, frame width included; that a claim
  asks for the source's channel count first and never for `S24Low`; that a source wider than the
  device is still asked for; which errors end a claim outright; and that anything else is I/O
  under the caller's context.

Each was confirmed to fail against a mutation of the code it covers. The claim itself, the
default-first listing and the writer loop need a device, and the manual runs above are their
coverage.

### Phase 6: macOS exclusive: moved to #113

Out of this plan, since there is no Mac to test it on. #113 carries the design notes.

### Phase 7: Polish

1. **Take the position lead out.** Subtract the negotiated device latency: cpal's
   `buffer_size`, `snd_pcm_delay`, or the padding. This is the audio-stack.md bullet that
   deferred the change to this feature.
2. A **Now Playing quality chip** (verdict + rate) that opens the signal path panel.
3. Advanced knobs: period / buffer, resync delay, and WASAPI **compatibility (polling) mode**
   for USB drivers that stutter under event mode. Each knob is bounded by the device's limits
   and reported back as negotiated.
4. Optional, off by default: **hardware volume** (ALSA simple mixer, `IAudioEndpointVolume`),
   so the slider can move without breaking the claim. It may be better as its own issue.
5. Docs:
   - README feature list and CLAUDE.md (AudioOutput ownership, the lock order).
   - `.claude/rules/audio-stack.md` (reshape, the exclusive backends, the verdict).
   - `unsafe-rust.md` already carries Phase 5's MMCSS row.
   - Then delete this doc.
6. **Carried from Phase 5's findings:** a refusal names the device that refused, and
   `FormatRefused` says "float" for a float source.
7. **Found in Gate A:** a toast when a claim falls back, and reclaiming a disconnected device as
   soon as it is listed again rather than at the next track start.
8. **Later: the chip in Shared mode too.** Until then it shows only while the Output Mode is
   Exclusive, because shared output always reads Converted and a red chip there would read as a
   fault on every default install. The later step shows it always:
   - in Shared mode, a neutral grey "Shared · 48 kHz" with no verdict colour;
   - under Exclusive, the verdict colours as now, red kept for a real conversion or a fallback.

   It needs a fifth label and a grey brush the chip takes as an input, like its others. The
   verdict itself doesn't change: the chip picks its words from the output mode and the verdict
   together.
9. **Later, optional: a Details button on the refusal toast**, opening the Output card through the
   chip's "open Settings on a tab" helper. The toast's action row already routes by
   `action_kind`, so it is a kind and a handler.

Hardware volume stays in this phase, on both platforms. The quality chip goes in the Now Playing
view and in the bottom bar, where it takes an overflow toggle like the other trailing buttons, and
it shows only under Exclusive until item 8.

**Split by platform (2026-09-27).** The Windows machine can't compile `alsa.rs`, `reserve.rs` or
`realtime.rs`: there is no WSL, and `alsa-sys` rules out a cross check. So the work is split:
- **The Windows half** does everything that compiles on Windows: the engine, the voices, the cpal
  shared path (which also runs on Linux), settings, UI, i18n and `wasapi.rs`.
- **A seam change touches the Linux files blind and only mechanically**, and every such edit is
  listed below.
- **A backend that hasn't measured or offered something yet reports nothing.** So ALSA exclusive
  behaves as it did in Phase 4 until the Linux half fills it in.

Windows half, done (2026-09-28):
1. `ExclusiveRequest { device, shape, format, tuning }` becomes the one argument an exclusive
   `open` takes. `tuning` is the period and, on Windows, event against polling. Also the refusing
   device's name, and the float wording.
2. The period knob and WASAPI polling mode.
3. The position lead. Each backend reports its lead through `Feed`, a voice clamps the heard
   position at its anchor, and the crossfade and preload keep the pulled clock.
4. The resync knob.
5. **Gate A** (passed, see its result below), then a toast when a claim falls back, raised on the
   same once-per-refusal check the warning is, and reclaiming a replugged device.
6. **Hardware volume** (item 4), on WASAPI and through the shared half every backend answers to.
   Gate B's volume half passed, see below. The `wasapi_tests` owed since Gate A and the
   `OutputFlags` round-trip landed alongside it.
7. **The performance pass**, and the fix for 24-bit output playing fast on the ALC897 that it
   turned up. Both are below, after the 24-bit result they correct.

**What's left of Phase 7, by where it can be done.** Most of it is cross-platform. Only the WASAPI
pieces need the Windows machine. The Linux half is done, see below.

*Either platform:*
- **The quality chip** (item 2), shown only while the Output Mode is Exclusive; item 8 is the
  Shared-mode follow-up.
  - `components/quality-chip.slint`:
    - a verdict dot: green Bit-perfect, yellow Enhanced, red Converted and Fallback;
    - a short translated label: four new msgids in all six catalogs;
    - `SignalPathUi.device-rate`;
    - a `Tooltip` inside the component, which `tooltip_mounts.rs` requires.

    Its brushes are defaulted `in` properties, `MetaChip`'s idiom, and its hover eases a float,
    never a brush (`slint-pitfalls.md`).
  - Shown while something plays and `Settings.exclusive-supported && Settings.output-mode-idx ==
    1`, a Fallback included.
  - Mounts:
    - **The Now Playing view's header row**, after the spacer and left of the stars, on the
      `Player.np-*` brushes.
    - **The bottom bar**, before the trailing buttons, under `if !Settings.overflow-quality`. That
      needs a row in `components/now-playing/overflow-menu.slint`, a checkbox in
      `views/settings/overflow-menu-section.slint`, and the `"quality"` id seeded in
      `ui/appearance/install.rs` beside the other overflow ids.
    - **Not the miniplayer**, which can't navigate to Settings.
  - **A click opens Settings on the Playback tab** through one Rust helper:
    - `ui::settings::settings_page::open_on(ui, SettingsTab::Playback)`, extracted from the
      onboarding card's `wire_open_services`, which moves onto it.
    - Tab first (`set_tab_idx` + `invoke_tab_changed`), then `nav_transition::mark(Above)`, closing
      Now Playing, and nav 9 (`set_selected_index` + `invoke_persist_selected_index`).
    - Not in Slint: a `SettingsPage` global importing `Nav` would be a third global importing a
      sibling, which the root `CLAUDE.md` rules out.
  - Walks it has to pass: `translations.rs`, `nav_transition.rs`, `tooltip_mounts.rs`,
    `index_persist.rs`, and `hero_chips_tests.rs` (the `np-*` brushes).
- **`AudioOutput`'s once-per-refusal rule has no device-free seam**, since `AudioOutput::open`
  needs a device. Gate A's runs and the Linux half's are its coverage unless the rule moves
  somewhere a test can reach. The other tests owed since Gate A landed with the Linux half.
- **Items 8 and 9**: the chip in Shared mode, and the Details button on the refusal toast.
- **Item 5's docs**: the README feature list, the root `CLAUDE.md` (`AudioOutput` ownership, the
  lock order), and the rest of `.claude/rules/audio-stack.md` (reshape, the exclusive backends,
  the verdict; the two positions a deck reports are already there).

*Windows only:*
- **The Linux half's blind edits**, listed at the end of its result below, then a short re-run of
  Gate B's volume half and a replug with the device picker open.
- **The WASAPI alignment retry**, never run on a device: only a driver refusing an unaligned
  buffer (`AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`) takes that path, and neither test device does.
  Windows returns that code only to an event-driven claim, so the retry is the event mode's
  `stream_mode` at the buffer the driver named, which `wasapi_tests` pins.
- **Gate B's chip half, by hand, after the chip** (its hardware-volume half passed, see below):
  - It shows only under Exclusive, and its colour matches the panel.
  - A click from Now Playing and from the bar lands on Settings ▸ Playback.
  - The overflow toggle moves it into the menu.

*Linux only:* nothing.

**The Gate A way**, for the runs above:
- Write test tones as WAVs.
- With the app closed, set the `output_*` keys and `repeat_mode` in the dev build's
  `settings.json`.
- Launch `target\debug\Melodia.exe "<file>" …`, which makes the files the queue and plays from the
  top.
- Read `%APPDATA%\Melodia-dev\logs\melodia_rCURRENT.log` under `RUST_LOG=info` plus debug for the
  module in question. Debug on `melodia_engine` logs every `player:` action, which dates a track
  start.
- Files handed over together queue in name order, not in the order given, so name them to sort
  the way the run needs.
- For a Busy test, a second Melodia with `MELODIA_DATA_DIR` set to a scratch folder is a separate
  instance and can hold the device.
- Back up `settings.json` and `queue.json` first, and restore them after.

**Blind edits the Linux session had to check first**, which it did on 2026-09-28: clippy and the
whole suite passed on Linux before anything else changed. Steps 1 to 4 made the ones in `alsa.rs`
and `unsupported.rs` listed in their as-built note below. Step 6 made these:
- `alsa.rs`: `pub(super) const HARDWARE_VOLUME: bool = false`, an
  `ExclusiveStream::hardware_volume()` that reads `negotiated.hardware_volume`, and
  `hardware_volume: false` in its `Negotiated` literal.
- `unsupported.rs`: the same const, and `hardware_volume()` as `match *self {}`.
- `melodia-integrations`' `mpris_backend/interface.rs` imports `amplitude_to_volume` from
  `engine::state`, where `media_controls::volume_percent` moved once the device-volume task became
  its second consumer.

**As built (2026-09-28), Windows steps 1 to 4:**
- **The blind edits, exactly.** `alsa.rs`:
  - `open(request: &ExclusiveRequest, feed, held)` in place of `open(device, shape, source, feed,
    held)`, resolving `request.device`.
  - `configure(pcm, request)` destructures the request, and asks the card for
    `frames_in(tuning.period, shape.rate)` where it asked for the fixed 20 ms `PERIOD`. So the period
    chips reach ALSA before the Linux half bounds them; `set_period_size_near` still clamps.
  - `ClaimError::FormatRefused { format: source }`, was `{ bits: source.bits }`.
  - `pub(super) const POLLING: bool = false`.
  - **Not mechanical:** `Writer::run` logs a stopped writer at `info`, was `warn`. A lost card is
    recovered from, and `tasks::audio_health` warns only when the recovery runs out.

  `unsupported.rs`: the same `open` signature, and `POLLING = false`.
- **One `Lead` cell per `AudioOutput`**, cleared on every reopen and park. Both cpal builders feed
  it the host's `playback - callback` plus the block just written; the WASAPI writer feeds it the
  frames written less `IAudioClock`'s position. The crate exposes no `GetStreamLatency`, so the
  device's own latency past its clock isn't in it.
- **`Voice::heard` scales the lead by speed** into media time, and never reads before its anchor.
- **Polling is a timer at half a period over a four-period buffer.** No alignment refusal reaches
  it: Windows returns `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED` only to an event-driven claim, whose
  buffer is its period, so only that mode retries at the buffer the driver named.
- **The period is five chips**, 5 to 100 ms, and `ExclusiveTuning::new` clamps to 2 to 100 ms. A
  hand-edited period off the chips selects none.
- **The resync hold is a slider up to `MAX_RESYNC_HOLD` (1 s)**, applied on release. Nothing
  reopens for it; the next rate change hears it.
- **A refusal warns once per distinct reason and device**, then logs at debug until the output goes
  shared, the user makes a new output choice, or a claim takes after a disconnect, so a second
  unplug is reported too. The device is looked up through `exclusive::devices()` rather than
  carried on the error.
- **Recovery logs at `info`, and only a recovery that runs out warns.** `tasks::audio_health` logs
  the loss and the stall at `info`, as both writers do the stopped stream, since an unplug or the
  system moving its default output recovers in a blink. The give-up line carries the last attempt's
  error, and `AudioOutput::open_shared` names the default device it tried at debug on each failure.

**Result, Gate A.** Scripted runs on 2026-09-27, Windows 11, reading the log. The app was driven
by its command line (a forwarded file replaces the queue and plays) over three generated tones:
44.1 kHz 16-bit, 48 kHz 16-bit and 48 kHz float.
- **Rate changes:** on both the ALC897 and the UMC22, 44.1 and 48 kHz opened as `Exclusive(S16)`,
  and each rate change reopened at the boundary rather than staging gapless.
- **Refusals name the refusing device:** the float tone was refused by both cards, logged as "a
  32-bit float source". On the UMC22 the fallback named the UMC22 while the audio went to the
  ALC897. Clearing the ALC897's exclusive-mode box fell back as `NotAllowed`, naming it.
- **Lead**, measured as pulled less heard:
  - about 39 ms exclusive at the 20 ms period;
  - 10 ms at 5 ms, which the ALC897 took as 224 frames;
  - 200 ms at 100 ms;
  - 75 to 80 ms polled on the UMC22 at 10 ms, which it took as 415 frames at 44.1 kHz;
  - 42 ms shared.
- **Polling:** the UMC22 played polled with no warnings, stalls or losses.
- **Resync:** at 1000 ms the clock held at 0 for a second after each rate change, and at 0 ms it
  started at once. The heard position stayed on the anchor until the pulled clock passed it.
- **Pause:** a media-key pause and resume kept the claim with no reopen, and the heard position
  never went backwards.
- **Unplug, by hand:** five unplugs of the UMC22 in polled mode fell back to the ALC897 within
  40 ms of the loss being seen, and each replug reclaimed the UMC22 without being asked. One more,
  on 2026-09-27 at 23:32, didn't; see Open questions.

**As built (2026-09-28), Windows step 5:**
- **The refusal toast** (`ToastKind::ExclusiveRefused`) is raised in `AudioOutput::fall_back` on
  the check that decides the warning, so it fires once per distinct reason and device.
  - Its detail is the refusing device's name.
  - Title and body are translated through two `Settings` pure callbacks.
  - It points at the Output settings for the reason. The reason's translated words live in the
    Slint panel and can't ride the toast channel.
  - It auto-dismisses after 6 s, since the music carries on shared.
- **The replugs in Gate A came back only by accident.** Windows moved its default output to the
  replugged UMC22, which knocked out the shared fallback and made the recovery re-ask for the
  claim. With the ALC897 left as the Windows default, a replug did nothing until the next track
  start, which a gapless album never reaches. Now `tasks::audio_health` polls
  `PlaybackEngine::disconnected_device_returned` once a second. It lists devices only while a
  `NotConnected` fallback stands, and reopens as soon as the chosen device is listed again,
  mid-track. Tested by hand with the ALC897 as the Windows default.
- **Only `NotConnected` is polled for.** Retrying a busy or refused device would keep taking it
  from its holder, which on Linux is the session manager. Those still wait for the next track
  start.

**As built (2026-09-28), Windows step 6, hardware volume:**
- **The toggle is part of the request** (`ExclusiveRequest::hardware_volume`), so a change reopens
  the claim, as the period and polling chips do. This retires "toggling it reopens nothing". A live
  switch between the voices' gain and the device's can play a period at the wrong level. At 5 %,
  turning it on would send unity samples to a device still at full volume for up to 100 ms.
- **`Negotiated::hardware_volume` is the capability and the route in one.** It is true only on a
  claim that asked and found a control in hardware. The dB range stays inside the backend.
- **There is no `ExclusiveStream::set_device_volume`.** The level rides a cell on `Feed` that every
  stream holds, like `Lead`, so a reopen claims at the level already set.
- **`AudioOutput::voice_gain` is the one rule.** The voices run at unity while the device carries
  the level, except at zero, which stays a silence in the voices.
  - The device's mute switch is never touched. At zero the device goes to its floor, so a rise
    from silence starts low.
  - `PlaybackEngine::set_volume` and the three track starts hand the amplitude to the output
    before any reopen, and take the gain after it.
- **`reopen_routed` wraps every reopen.** It holds the voices at the software gain across the
  reopen, and sets unity after only where the new stream took the control. So a change of route,
  a fallback to shared included, dips rather than jumps.
- **The level is Windows' own percentage** (`SetMasterVolumeLevelScalar`), not dB.
  - It was built first as `20·log10(amplitude)` in dB. Gate B found Melodia at 50 % reading 67 %
    in Windows, whose slider is its own audio taper over the device's range.
  - Now the two sliders read the same number. A position follows Windows' curve rather than the
    voices' linear one.
- **Only a control in hardware.** Without one, the endpoint volume is the audio engine's, which
  exclusive mode bypasses.
  - The query is `wasapi`'s safe `query_hardware_support`.
  - The rest goes through `windows` 0.62.2, the copy `wasapi` and cpal resolve: five `unsafe` COM
    calls in `output/endpoint_volume.rs`, listed in `unsafe-rust.md`.
- **The original level is put back at release**, and outlives a release that couldn't. It is kept
  by endpoint id until it is back, so an unplug and replug still restores what the device had
  before the first claim. Windows persists an endpoint's level, and the replug would otherwise
  report Melodia's as its own. A crash still leaves Melodia's level.
- **A claim never turns the device up past what the system chose.** Where Windows has the device
  quieter than Melodia's slider, `EndpointVolume::take` leaves it there and the claim reports the
  level through `ExternalVolume`, so the slider follows it down. Without that, a slider left at
  full in shared mode blasts a device Windows keeps low the moment the claim takes the control.
  - A device still at the level a release put back is resuming, and takes Melodia's level. Every
    reopen releases first, so capping there would drag the slider down at each rate change.
  - Only a release in this run counts. The first claim after a launch caps again.
- **Every COM call is on `wasapi-out`.** The level is set after a write, at most every 20 ms, since
  each set is a call into the audio service. A failure ends the stream like a failed write, and
  the reclaim decides again.
- **The system moving the control moves Melodia's slider**, added after Gate B's first run.
  - The writer reads the level every 250 ms, against what it read back after its own last set, so
    the device's rounding never reads as a move.
  - A level the device already sits at, within half a percent, isn't written back
    (`endpoint_volume::already_at`). A move handed back returns as the slider's rounding of it, and
    writing that would undo any move the system made meanwhile, which the read-back then took as
    Melodia's own and never reported. Found in review after Gate B, not reproduced before the fix.
  - A move goes through `Feed`'s `ExternalVolume` (a one-slot atomic and a `Notify`) to
    `tasks::device_volume`. That applies it the way an OS media panel's volume is applied,
    `settings.json` included.
  - `media_controls::volume_percent` moved to `engine::state::amplitude_to_volume` for its second
    consumer.
  - Windows' mute button isn't followed.
- **Make Bit-Perfect leaves a volume on the device alone** and only unmutes. `can-reset` offers it
  for the volume only while muted. The live half now returns the level it left and runs in the same
  blocking task as the persist, which closes a race between the two.
- **Quitting closes the output** (`PlaybackEngine::close_output`, after `save_state_on_exit`).
  `process::exit` runs no destructor, and a parked output would be reopened by a track starting
  while the monitor still runs. On Linux this also hands the card and its reservation back at quit.

**Result, Gate B's volume half.** By hand on 2026-09-28, Windows 11, reading the log:
- **Both test devices have a control in hardware.** Every claim on the UMC22 and the ALC897 read
  `hardware_volume: true`.
- **Percentages:** Melodia's slider and Windows' read the same at every position, and the panel
  stayed Bit-perfect while the slider moved.
- **Silence:** 0 % and mute went silent at once, and a rise from them didn't blip.
- **The toggle:**
  - Mid-track it reopened with a short gap and no loud blip.
  - Off, it put Windows' level back, and Melodia kept its own.
- **Release:** closing Melodia put Windows' level back.
- **Unplug:** the UMC22 fell back to the ALC897 within 40 ms of the loss, and was reclaimed on
  replug. A stop afterwards put its level back. The loss logs `0x80004005` (E_FAIL), as every
  unplug has since Gate A.
- **Sync back:** Windows' slider and volume keys moved Melodia's.
- **Sync back, re-run after the write-back fix:**
  - A held volume key moved both sliders with no step back.
  - The flyout slider stayed where it was released.
  - Event mode at 5 ms, dragging Melodia's slider, played with no audible dropout. That one is by
    ear: the WASAPI writer counts no underruns (see Open questions).
  - Toggling off and quitting both still put Windows' level back.
- **Not run:** a device with no control in hardware, since neither test device lacks one.
  `voice_gain`'s test covers the rule that keeps the voices carrying the level there.

The tests landed are:
- `wasapi_tests`: `stream_mode` under both drives, and `hns` against `from_hns` at the period
  chips and past both ends.
- `output/mod_tests`: `voice_gain`, and `endpoint_volume_tests`: `level_for` across `0..=1`, past
  both ends and at NaN, and `already_at` either side of half a percent.
- `device_tests`: `ExternalVolume` hands over a report, waits with none, keeps only the latest of
  two, and wakes a take already waiting.
- `signal_path_tests`: on the device, any audible level grades clean, while zero and mute stay
  the user's.
- `settings_tests`: an output choice comes back from `settings.json` whole, a hand-edited period is
  held to the claim's range, and a file from before the setting leaves it off.
- `main_order_tests`: the output is closed before either way out of `main()`.

Each was confirmed to fail against a mutation of the code it covers. Taking the control, watching
it and restoring it need a device, and the Gate B runs above are their coverage.

**Result, 24-bit and high-rate output through WASAPI**, which Phase 5 left unrun. A scripted run on
2026-09-28, Windows 11, run the Gate A way on the ALC897. The UMC22 is 16-bit at 48 kHz or below,
so it would refuse every tone.
- **The setup:** generated 440 Hz stereo tones at −20 dBFS, six seconds each:
  - 24-bit at 44.1, 96 and 192 kHz;
  - a 16-bit 192 kHz control.

  Polled, 20 ms period, Hardware Volume on.
- **Each 24-bit tone claimed at its own rate as `Exclusive(S24Packed)`**, the ladder's first 24-bit
  rung, with `hardware_volume: true`, no fallback and no warning.
- **Corrected by the performance pass below: every one of those claims played a third fast.** The
  ALC897 took `S24Packed` only in a spelling its driver misreads. Each six-second tone moved on
  after 5.0 s, which this run read as a pass. They now claim `Exclusive(S24High)` at speed.
- **Periods:** the ALC897 took 831 frames at 44.1 kHz against the 882 asked for, and exactly the
  1920 and 3840 asked for at 96 and 192 kHz.
- **Each rate change reopened at the boundary**, as in Gate A.
- **The 16-bit control:** its reopen keeps the rate, so it logs at debug, which the run didn't
  record for the engine. It logged no refusal and played through.

**As built (2026-09-28), the Windows performance pass.** A review of this branch for CPU, memory
and latency, the Windows half measured. The writer's loop was already clean: per period it
allocates nothing, takes one uncontended `try_lock` and logs nothing. What changed is on the
control side:
- **A refused claim is retried at a track start only where the refusal can pass**
  (`FallbackReason::may_pass_later`): busy, reserved, not allowed, not connected, or a fault. A
  rate, channel count or format the device lacks is refused again, and retrying it tore down and
  reopened the shared stream at every skip, under the decks and output locks.
- **A claim that already holds the next track keeps playing** (`AudioOutput::serves`). A same-rate,
  same-shape track in another format stays on the open claim where the claim's format is on the new
  source's ladder, so a 16-bit track after a 24-bit one plays gapless. The reverse still reopens.
  `request()` stays what the open stream was asked for.
- **`OutputStatus` is a lock-free cell** for "parked" and "waiting on a disconnected device",
  shared like `Lead`.
  - The 250 ms health tick read the parked flag through the output mutex, on one of the runtime's
    two workers, and a reopen holds that mutex.
  - The 1 s reclaim tick does nothing outside a disconnect. It used to hop to the blocking pool
    every second, for every user.
  - `close_output` parks on the way, so the stall watch doesn't take the closed output for a lost
    one.
- **A boot under Exclusive starts parked** (`AudioOutput::open_parked`), where it opened the
  default device shared only to close it before anything played. `AppState::init` reads the
  settings first.
- **A chosen device is resolved by id** (`get_device`, then an active-state check) rather than by
  listing and naming every endpoint on each claim. Only `ERROR_NOT_FOUND` or an inactive device is
  `NotConnected`. Any other failure is I/O, so a COM error can't set the reclaim poll going on a
  device that is plugged in.
- **One COM call per polled wake:** the room is the cached buffer size less `GetCurrentPadding`.
- **Deferred until measured:**
  - a per-format encoder, since `f64::round` is a CRT call on baseline x86-64;
  - moving transport ops that can reopen off the runtime's two workers;
  - the endpoint-volume calls on a thread of their own (Gate B heard no dropout at 5 ms);
  - fewer `GetPosition` calls.

  Older than this branch and out of its scope: the file decoder is still pulled on the MMCSS
  thread.

**Found in the performance pass: 24-bit output on the ALC897 played a third fast.** A 20-second
24-bit tone ended after 15.5 s, events and polled alike, where 16-bit took 20.5 s. A throwaway probe
that claimed each spelling and drained silence found the cause:
- The Realtek HD Audio driver refuses 24-bit packed in `WAVEFORMATEXTENSIBLE`: `Initialize`
  answers `0x88890008`.
- It takes the same format in the short `WAVEFORMATEX` header, which the crate's quirk walk offers
  next, and drains it as 32-bit samples. Its own `IAudioClock` ran at 1.333 times wall time at 44.1
  and 96 kHz, so the track plays fast and garbled while every call succeeds.
- Windows defines the short header for 8- and 16-bit PCM only.
- `S24High` (a 32-bit container with 24 valid bits, extensible) is taken and drains at exactly wall
  time. `S32` and `F32` are refused.

The fix is in `wasapi.rs`, which now walks the respellings itself: the crate's order, less the
short header for integer PCM wider than 16 bits. The claim then moves down the ladder to `S24High`.
ALSA has no such header, so only the Windows backend changed.

**Result, the performance pass.** Scripted on 2026-09-28 the Gate A way, with 5 s tones at −30 dBFS:
- **The refusal retry:** on the UMC22, the first of two MP3s was refused once, the claim attempt
  and the shared fallback taking about 28 ms. The second neither retried nor reopened. A 16-bit WAV
  after them claimed `Exclusive(S16)`. After the queue stopped, an MP3 was refused again at debug,
  and the one after it wasn't retried.
- **Busy is still retried:** a second Melodia held the ALC897.
  - The first's claim fell back as Busy (`0x8889000A`).
  - The next track retried and was Busy again.
  - Once the second quit, the next track claimed `Exclusive(S16)`.
- **A format change:** on the ALC897, 24 → 16 → 24 → 16-bit ran on one claim, every boundary
  gapless and 5.0 s apart. 16 → 24 still reopened, after "Not staging … gapless".
- **24-bit speed:** a 20 s 24-bit tone now claims `Exclusive(S24High)` and ends after 20.55 s under
  events and 20.52 s polled.
- **Not connected:** a device Windows knows but that isn't present, and an id that never existed,
  both fell back as `NotConnected` rather than I/O. The reclaim poll listed the devices once a
  second while that fallback stood, and stopped when the output parked.
- **The boot:** with Exclusive saved it logs "parked until the first play claims the device". With
  Shared saved it still opens at launch.
- **Polled mode:** 16-bit played in real time on both devices, with no stall.
- **Quits:** none of about a dozen logged "output lost; reopening".
- **Unplug and replug**, by hand: recovered as before.

The tests landed are:
- `claim_tests`: which reasons `may_pass_later`.
- `output/mod_tests`: `claim_serves` across format changes both ways, a float source on `S32` and
  `F32`, and never for a request that differs in more than the format.
- `wasapi_tests`: the short header is offered only for 16-bit PCM and float.

Each was confirmed to fail against a mutation of the code it covers. The status cell, the parked
boot and the lookup by id need a device, and the runs above are their coverage.

**Linux half, done (2026-09-28):**
1. The lead, from `pcm.delay()` after each write.
2. The period bound.
3. **Hardware volume, at parity with Windows**, through the same rules rather than a copy of them.
4. **The card's own level with Hardware Volume off**, which is Phase 4's blind spot, answered on
   Linux.
5. Step 5's and the performance pass's shared code, run on Linux for the first time.
6. Found in the replug run: the device picker's list went stale.

Item 5's docs remain. Delete this doc only once items 8 and 9 are done or moved to their own
issues.

**As built (2026-09-28), the Linux half:**
- **The volume rules are shared, not copied.** `output/hardware_volume.rs` holds everything
  `endpoint_volume.rs` had that isn't Windows': never raising, resuming, the originals kept until a
  release puts them back, the follow pacing, and the move watch. It is generic over a
  `VolumeControl` that reads and sets the fraction the system's own slider shows.
  `endpoint_volume.rs` keeps the COM half and `alsa_volume.rs` is the ALSA one. The same five
  `unsafe` calls sit under the same three attributes, so `unsafe-rust.md`'s counts hold.
- **Which element: `Master`, else the card's only element with a playback volume, and only on a
  claim of the card's device 0.** The list's "then `PCM`, then the first" would have played at full
  volume on this machine:
  - `PCM` on both HD Audio cards is alsa-lib's softvol user control (`HDA-Intel.conf`), which does
    nothing on `hw:`.
  - HDMI and S/PDIF devices share their card's mixer, and no element on it acts on them.
  - The ALC897 lists a dozen elements with a playback volume, mic loopbacks included.

  The alsa crate can't tell a user control from a driver's without new `unsafe`, so the rule is
  structural. The ALC897 takes `Master` (−65.25 to 0 dB) and the PCM2902 `PCM` (−128 to 0 dB).
- **The curve, corrected.** Past 24 dB of range, `volume_mapping.c` is a cube-root taper,
  `10^((dB − max)/60)`, with the floor moved to zero unless it is `SND_CTL_TLV_DB_GAIN_MUTE`. It is
  not linear amplitude, as the list said. A set lands on the nearest step, so the two read the same
  whole percent.
- **Each read drains the mixer's events first**, since the simple mixer caches an element until its
  events are read and a move made elsewhere arrives as one. Draining never blocks, which was checked
  on both cards.
- **The writer owns the volume, and closes the card before putting the level back.** That way no
  buffered period plays at the original, and a granted `RequestRelease` hands the card back at its
  own level. A reopen on the same card carries the reservation but, as on Windows, each stream
  takes and restores its own volume, and the resuming rule stops that dragging the slider.
- **`Negotiated::device_level`** is the blind spot answered on Linux. On a claim whose device's
  control doesn't carry the volume, it reports where the system left that element and its switch.
  - **It grades nothing**, departing from "stops reading Bit-perfect" above. The samples reach the
    device untouched, and Gate B already grades a level on the device clean.
  - The Volume row names it instead: "39% (the device at 71%)" or "39% (the device muted)".
  - WASAPI reports `None` until its read is written.
- **The lead** is `pcm.delay()` after each write, over the rate. A read that fails leaves the last
  one standing.
- **The period is clamped** so the buffer's four periods fit the largest buffer the card offers.
- **The rules take the clock as a parameter** (`follow_at`, `take_move_at`), so their pacing is
  tested without a sleep.
- **The device picker lists the cards when it is about to be read**: on the device row's mount and
  on the dropdown opening (`Dropdown.about-to-open`, `Settings.list-output-devices`). It listed them
  at boot and on a mode change only, so a card plugged in after launch never appeared, on either
  platform.

**Result, the Linux half.** Runs on 2026-09-28 against the UMC22 (PCM2902) and the ALC897, reading
`amixer -M`, `/proc/asound`, MPRIS and the log. The unplug and replug were by hand.
- **Hardware volume:**
  - A first claim with Melodia at 80 % over the card's 71 % left the card there, and the slider
    followed it down.
  - Melodia's slider at 50, 25, 37, 10 and 60 % read the same on `amixer -M`, 37 as 36 since the
    card steps in whole dB. `amixer` moves to 44, 20, 79 and 34 % moved Melodia's slider, and none
    was written back.
  - 0 % put the element at its −128 dB floor with the switch left on.
  - Stop and quit put the element back at 71 % and the card back to the session manager under its
    own name. Play after Stop, and a rate change mid-queue, resumed Melodia's level. Between the two
    streams of a rate change the card sits at its original for about 50 ms, before the new writer
    starts.
  - The ALC897's `Master` followed Melodia's 39 % to 38 %, its nearest step, and went back to
    100 % at stop.
  - An HDMI claim took no hardware volume and left the softvol `PCM` alone.
  - Turning it off mid-track reopened with the card back at its own level. The same slider position
    then plays about 7 dB louder, the card's curve and the voices' amplitude disagreeing there. By
    design, as on Windows.
- **The card's own level:** the claim read 71 %, and a switch muted over a rate change read
  `muted: true`. Both rows read as worded. **The session manager unmutes a card when it takes it
  back**, so a mute set during a claim shows only over a reopen that keeps the reservation.
- **The lead:** the published position sat within 10 to 15 ms of what the card had played, where
  the card held 76 to 95 ms.
- **Periods:** every chip at 192 kHz on the ALC897 left four periods in the buffer, 960 to 19200
  frames. The bound never engaged, since neither card's largest buffer is short of four 100 ms
  periods.
- **Formats:** 16 → 24-bit reopened, and 24 → 16-bit stayed on the claim gapless. A 24-bit
  192 kHz file claimed `Exclusive(S32)`.
- **Step 5 and the performance pass:**
  - A float after a float asked the card once. The refusal warned and toasted once, and after a
    stop logged at debug.
  - `Reserved`, against a second Melodia holding the card, warned once, was retried at the next
    track, and claimed once the holder quit.
  - A boot under Exclusive started parked.
  - An unplug logged the loss and the recovery at info, and the fallback warned once with its toast.
    A replug reclaimed the card within a second, straight past the session manager, and a second
    unplug warned again.
  - Quitting handed the card and its reservation back.
  - No underruns in any run.
- **The first replug of the session never reached the kernel.** No USB enumeration followed it,
  which read as a failed reclaim. The unplug itself logs ALSA's `EBADFD` as "Unknown errno (77)",
  at info.
- **Not run:** the cpal shared lead under PipeWire.

The tests landed are:
- `hardware_volume_tests`, over a fake control and so on both platforms: never raising, resuming, a
  level the system chose after a release, a failed restore kept for the next claim, the follow
  pacing, no write-back over a system move, and a move reported once but never the read-back of a
  set. The two tables from `endpoint_volume_tests` moved here.
- `alsa_volume_tests`: the element pick, the scale from the stated ranges, readings against
  `amixer -M`, and a level reading back as itself.
- `output/mod_tests`: `DeviceLevel`'s rounding. `signal_path_tests`: a device level never moves the
  grade.
- The ones owed since Gate A:
  - `voice_tests`: the ear trails by the lead, never reads before the anchor, scales the lead by
    speed, and re-anchors on a resume but not on a `play` over a playing voice.
  - `handlers_tests`: the tick publishes the heard position, and the preload and the crossfade time
    against the pulled one.
  - `claim_tests`: the float wording.

Each was confirmed to fail against a mutation of the code it covers. Opening a mixer, the device-0
rule and the writer's loop need a card, and the runs above are their coverage.

**Blind edits the Windows session must check first.** CI's `clippy-windows` and `test-windows`
compile them on push:
- `endpoint_volume.rs` became the COM half: `EndpointControl` implementing `VolumeControl`,
  `take(device, id, volume)`, `activate`, and `EndpointVolume` as an alias for
  `HardwareVolume<EndpointControl>`.
- `wasapi.rs`: the import (`endpoint_volume::{self, EndpointVolume}`), `endpoint_volume::take`, and
  `device_level: None` in its `Negotiated` literal.
- `tests/endpoint_volume_tests.rs` is gone, its two tables now in `hardware_volume_tests.rs`.

Then a short re-run of Gate B's volume half, since the rules moved, and a replug with the device
picker open.

## Cross-cutting

- **Memory.** Each claim adds one writer thread and one staging buffer sized to a period.
  There is no ring and no new cache. A reshape frees the old stream before the new one opens.
- **Threading.** Backends never touch Slint. A reopen never runs on the UI thread, since opening
  a device can block.
- **Errors.** `ClaimError` is typed and user-facing. `error::describe` covers the logs.
- **Logging.** Nothing logs in `fill`, `encode` or a writer loop. The health counters exist for
  that.
- **i18n.** Every literal needs a msgid in all six catalogs. Device names and formats are data.

## Open questions

- Can shared output learn the card's own rate under PipeWire (the node's `clock.rate`), so the
  Sample Rate row stops reading clean when the graph resamples to a rate the card has?
- Should a long pause give up the exclusive claim, so other apps get the card back?
- ~~How long does the session manager take to let go of a card after a ReserveDevice1
  release?~~ It answers at once and closes the card a moment later, which is what
  `alsa::BUSY_WAIT` covers. See Phase 4's findings.
- Should the device picker show the names the desktop gives the cards, so the two lists agree?
- A 16-bit-only card refuses every lossy file, which decodes to float. Since the performance pass
  a refusal isn't retried for the same request, so a run of lossy files asks once. Each lossy file
  after a lossless one still asks, and on Linux takes the card from the session manager for a
  moment. Learning a card's formats once would skip claims that cannot work.
- **Should a claim check its own clock?** The ALC897 misread a spelling it had accepted, and only
  its clock running off wall time showed it. A writer could compare `IAudioClock` against the wall
  clock over its first second and move down the ladder when they disagree. The short-header rule
  covers the one case seen, so it isn't built.
- Once cpal 0.19's extension traits ship (#1220), can they replace `wasapi.rs` or add a native
  PipeWire exclusive stream (`PW_STREAM_FLAG_EXCLUSIVE` / `node.force-rate`)?
- **With Hardware Volume off, a Windows device with its own control still plays at its Windows
  level**, while the panel reads Bit-perfect. It is Phase 4's blind spot on Windows: exclusive mode
  bypasses the audio engine, not the device. Reading the endpoint level at claim, as `take` already
  does, would let the Volume row name it. Linux does since its half of Phase 7, through
  `Negotiated::device_level`; WASAPI fills that field with `None`, and the read would go there.
- **A crash while the device carries the volume leaves Windows at Melodia's level.** Only a release
  puts the original back. Persisting the original would cover a crash, at the cost of a file
  written on every claim.
- **The WASAPI exclusive writer counts no underruns.** Only ALSA's writer and the cpal callbacks
  call `record_xrun`, so `audio_health`'s underrun line never fires for a WASAPI claim, and a
  dropout there is heard rather than logged.
- **Windows' mute button isn't followed.** Only the level is watched. Following it would take
  `GetMute`, a sixth COM call, and a decision about whether it mutes Melodia or only the device.
- **A claim made while muted or at 0 % escapes the never-raise rule.** The device goes to its floor,
  which raises nothing, and the unmute then `follow`s it straight to Melodia's slider, however low
  Windows had it before the claim. Capping it would mean carrying the pre-claim level as a ceiling
  until the user moves the slider.
- ~~A refusal should name the device that refused.~~ It does since Phase 7: `Negotiated.fallback`
  is a `Fallback` carrying the refusing device, looked up through `exclusive::devices()`, and the
  log line and the panel's Device row name it beside the device the audio went to.
- ~~`ClaimError::FormatRefused` says "a 32-bit source" for a lossy track.~~ It carries the whole
  `SourceFormat` since Phase 7, whose `Display` says "32-bit float".
- **Why did one unplug end in 15 seconds of silence?** On 2026-09-27 at 23:32 the UMC22 was
  unplugged while claimed. Every reopen attempt fell back and then failed to open the shared
  default, until the UMC22 came back and was reclaimed. Five other unplugs, across four sessions,
  recovered at once. What was ruled out:
  - the device picker resolving a pick to the wrong device;
  - a claim left unreleased after switching from the ALC897;
  - a Stop and Play between the claim and the unplug.

  The likeliest cause is Windows moving its default output between the two cards around a
  replug, leaving the fallback asking for a default that had just gone. The give-up line now
  carries the last attempt's error, so the next occurrence names its cause. If it is the default,
  the fix is for the fallback to try any other connected output when the default won't open.

