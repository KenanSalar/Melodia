# Bit-Perfect Output

Working doc. Delete it when the feature ships. Tracks issue #66.

Status: **Phase 1 done** (2026-09-24) · **Phase 2 done** (2026-09-25) · **Phase 3 done** (2026-09-25) · **Phase 4 done** (2026-09-27) · **Phase 5 done** (2026-09-27) · Phase 7 in progress, Windows half first (see its split) · macOS moved to #113 · Rewritten: 2026-09-23 (replaces the 2026-08-14 draft)

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
claim belongs with Phase 7's hardware volume.

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
  refuses the whole device ends the claim.
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
  24-bit or high-rate output (no such files at hand), and the alignment retry.

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

Hardware volume stays in this phase, on both platforms. The quality chip goes in the Now Playing
view and in the bottom bar, where it takes an overflow toggle like the other trailing buttons.

**Split by platform (2026-09-27).** The Windows machine can't compile `alsa.rs`, `reserve.rs` or
`realtime.rs`: there is no WSL, and `alsa-sys` rules out a cross check. So the work is split:
- **The Windows half** does everything that compiles on Windows: the engine, the voices, the cpal
  shared path (which also runs on Linux), settings, UI, i18n and `wasapi.rs`.
- **A seam change touches the Linux files blind and only mechanically**, and every such edit is
  listed below.
- **A backend that hasn't measured or offered something yet reports nothing.** So ALSA exclusive
  behaves as it did in Phase 4 until the Linux half fills it in.

Windows half, in order (1 to 4 done 2026-09-28):
1. `ExclusiveRequest { device, shape, format, tuning }` becomes the one argument an exclusive
   `open` takes. `tuning` is the period and, on Windows, event against polling. Also the refusing
   device's name, and the float wording.
2. The period knob and WASAPI polling mode.
3. The position lead. Each backend reports its lead through `Feed`, a voice clamps the heard
   position at its anchor, and the crossfade and preload keep the pulled clock.
4. The resync knob.
5. **Gate A** (passed, see its result below), then a toast when a claim falls back, raised on the
   same once-per-refusal check the warning is.
6. The quality chip.
7. Hardware volume: the shared routing plus `IAudioEndpointVolume`. Then **Gate B**.

**Blind edits the Linux session must check first**, running
`cargo clippy --all-targets --locked --workspace -- -D warnings` and then `cargo test`. They are in
`alsa.rs` and `unsupported.rs`. Steps 1 to 4 made the ones in the as-built note below; step 7 adds
the `Negotiated` literal's new fields and a no-op `set_device_volume`.

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
- **Polling is a timer at half a period over a four-period buffer.** After an alignment refusal
  the period is cut from the aligned buffer, so the buffer stays the size the driver named.
- **The period is five chips**, 5 to 100 ms, and `ExclusiveTuning::new` clamps to 2 to 100 ms. A
  hand-edited period off the chips selects none.
- **The resync hold is a slider up to `MAX_RESYNC_HOLD` (1 s)**, applied on release. Nothing
  reopens for it; the next rate change hears it.
- **A refusal warns once per distinct reason and device**, then logs at debug until the output goes
  shared. The device is looked up through `exclusive::devices()` rather than carried on the error.
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

**Linux half:**
1. The lead: after each `writei`, report `pcm.delay()` over the rate through
   `Feed::report_lead`. Also confirm the cpal shared lead on the ALSA host under PipeWire, where
   htstamp is zero but the delay still holds.
2. The period knob: bound it with `get_period_size_min/max`, read the buffer back, and check it in
   `hw_params`.
3. Hardware volume through the claimed card's simple mixer: dB via `get_playback_db_range` /
   `set_playback_db_all`, the playback switch for mute, and a restore at release. Also read the
   mixer at claim, so a muted or attenuated card stops reading Bit-perfect (Phase 4's blind spot).
4. Manual runs on the ALC897 and the PCM2902, then the tests.
5. Item 5's docs, then delete this doc.

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
- A 16-bit-only card refuses every lossy file, which decodes to float, and each refusal still
  takes the card from the session manager for a moment. Learning a card's formats once would
  skip claims that cannot work.
- Once cpal 0.19's extension traits ship (#1220), can they replace `wasapi.rs` or add a native
  PipeWire exclusive stream (`PW_STREAM_FLAG_EXCLUSIVE` / `node.force-rate`)?
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

