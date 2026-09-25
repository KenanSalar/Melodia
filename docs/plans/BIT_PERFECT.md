# Bit-Perfect Output

Working doc. Delete it when the feature ships. Tracks issue #66.

Status: **Phase 1 done** (2026-09-24) · **Phase 2 done** (2026-09-25) · Phase 3 next · Rewritten: 2026-09-23 (replaces the 2026-08-14 draft)

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
- **cpal's CoreAudio host already switches the device's nominal rate and physical format** to
  whatever config the stream is built with. On macOS, "match the file rate" is nearly free, and
  exclusive comes down to hog mode around a cpal stream. The old draft's own IOProc is not needed.
- **The old spine ("rebuild the decks against a new mixer") is replaced by reshaping in place.**
  See Phase 1. The decks, their fade cells, a staged gapless source and the position all survive
  a reopen, because only the stream is replaced.
- **Wrapper crates keep the new `unsafe` count at zero.** `alsa` 0.11 is safe. The `wasapi`
  crate wraps COM. `coreaudio-rs` 0.14.2 (already in the lock through cpal) has safe
  `toggle_hog_mode` and `get_hogging_pid` helpers. `objc2-core-audio` and a raw `windows`
  backend would each add dozens of sites against `unsafe_code = "deny"`.
- **No ring buffer.** A backend's writer thread owns `MixerPull` and calls `fill` directly. The
  old draft's per-sample ring-drain critique is moot.
- **Packaging is unaffected.** `libasound` is already linked through cpal.

## How the best implementations do it (mechanisms, no attribution)

Surveyed: the sibling checkouts under `~/Development`, plus current docs and changelogs.
- Exclusive claims: ALSA `hw:` with resampling off. WASAPI exclusive, **event-driven**,
  `buffer == period`, integer PCM tried before float (many drivers refuse float in exclusive),
  `IsFormatSupported == S_OK` exactly, a fresh client on `BUFFER_SIZE_NOT_ALIGNED`, MMCSS on
  the writer thread. CoreAudio hog mode with a read-back guard, because a set is a toggle.
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
  device.rs     the cpal backend (shared, plus macOS exclusive via hog.rs). Ladder prefers the requested shape
  mixer.rs      + MixerPull::reshape, MixerPull::hold (resync silence)
  voice.rs      + VoicePull::reshape. The control side's device shape becomes a shared cell
  convert.rs    unchanged
  encode.rs     NEW: DeviceFormat + the ladder + the one f32 -> device-bytes conversion for owned backends
  claim.rs      NEW: ClaimError (typed fallback reasons, a local std::error::Error)
  alsa.rs       NEW  #[cfg(target_os = "linux")]
  reserve.rs    NEW  #[cfg(target_os = "linux")]   ReserveDevice1 over zbus::blocking
  wasapi.rs     NEW  #[cfg(target_os = "windows")]
  hog.rs        NEW  #[cfg(target_os = "macos")]   hog mode + device-id lookup via coreaudio-rs
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

Each phase leaves the tree shippable. Phases 4 to 6 are independent: the seam falls back to
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
gapless. On macOS, Audio MIDI Setup follows the file. On Linux, the entry check's answer holds.

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

### Phase 3: The signal path panel and "Make bit-perfect"

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

### Phase 4: Linux exclusive: ALSA `hw:`, ReserveDevice1, device picker

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

**Exit:**
- `/proc/asound/card*/pcm*p/sub*/hw_params` shows the file's rate and format.
- Other apps go silent while exclusive holds the card, and recover when the mode is turned off.
- A card held by another client falls back with the right reason.
- The resync default is tuned on real hardware.

**Tests after the go:**
- `encode` round-trip units for every rung, including sign and full-scale saturation.
- `crates/melodia/tests/bit_perfect.rs`: 16-bit and 24-bit WAV fixtures (generated with
  ffmpeg into `tests/assets/`, with `.gitattributes` binary lines) go through
  `FileDecoder` → the full chain → `mixer::pair` → `encode`. Assert the result equals the
  file's PCM, and that enabling EQ breaks the equality.

### Phase 5: Windows exclusive: WASAPI through the `wasapi` crate

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

### Phase 6: macOS exclusive: hog mode around the cpal stream

1. **`hog.rs`**, over `coreaudio-rs` pinned to the lock's `0.14.2`:
   - Map the chosen cpal device to its `AudioDeviceID` by name. Verify this against
     `coreaudio-rs`'s helpers.
   - Take hog mode, guarded by `get_hogging_pid` (the set toggles, so re-read to confirm).
   - Build the cpal stream at the file's shape. cpal sets the nominal rate and physical format
     and waits for the settle.
   - Release hog mode **after** the stream drops.
2. A device that dies arrives as `DeviceNotAvailable` → Phase 1's recovery.

**Exit:** other apps go silent. Audio MIDI Setup shows the file's rate and format. Turning the
mode off restores system audio. Unplugging recovers.

### Phase 7: Polish

1. **Take the position lead out.** Subtract the negotiated device latency: cpal's
   `buffer_size`, `snd_pcm_delay`, or the padding. This is the audio-stack.md bullet that
   deferred the change to this feature.
2. A **Now Playing quality chip** (verdict + rate) that opens the signal path panel.
3. Advanced knobs: period / buffer, resync delay, and WASAPI **compatibility (polling) mode**
   for USB drivers that stutter under event mode. Each knob is bounded by the device's limits
   and reported back as negotiated.
4. Optional, off by default: **hardware volume** (ALSA simple mixer, `IAudioEndpointVolume`,
   CoreAudio device volume), so the slider can move without breaking the claim. It may be
   better as its own issue.
5. Docs:
   - README feature list and CLAUDE.md (AudioOutput ownership, the lock order).
   - `.claude/rules/audio-stack.md` (reshape, the exclusive backends, the verdict).
   - An `unsafe-rust.md` row only if a site appeared after all.
   - Then delete this doc.

## Cross-cutting

- **Memory.** Each claim adds one writer thread and one staging buffer sized to a period.
  There is no ring and no new cache. A reshape frees the old stream before the new one opens.
- **Threading.** Backends never touch Slint. A reopen never runs on the UI thread, so
  CoreAudio's rate settle blocks an engine thread instead.
- **Errors.** `ClaimError` is typed and user-facing. `error::describe` covers the logs.
- **Logging.** Nothing logs in `fill`, `encode` or a writer loop. The health counters exist for
  that.
- **i18n.** Every literal needs a msgid in all six catalogs. Device names and formats are data.

## Open questions

- Does opening cpal's ALSA default at the file rate move the PipeWire graph? (Phase 2 entry check)
- Should a long pause give up the exclusive claim, so other apps get the card back?
- How long does the session manager take to let go of a card after a ReserveDevice1 release?
- Once cpal 0.19's extension traits ship (#1220), can they replace `wasapi.rs` or add a native
  PipeWire exclusive stream (`PW_STREAM_FLAG_EXCLUSIVE` / `node.force-rate`)?

