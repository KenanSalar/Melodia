---
paths:
  - crates/melodia-audio/src/player/source/**/*.rs
  - crates/melodia-playback/src/player/playback/**/*.rs
  - crates/melodia-engine/src/player/engine/**/*.rs
---

# The audio stack

The playback state machine, the DSP chain, and the output layer under both. Each module's `//!`
argues its own design and each constant is justified at its definition; read those first. This
file holds what spans crates, or breaks from outside the tier that owns it.

**Three tiers, and the direction never reverses**: `source/` (the `AudioSource` vocabulary, one
Symphonia and what feeds it), `playback/` (the DSP chain and `output/` under it), `engine/` (the
state machine, the queue, the actions and the backend). `engine` reads `playback` reads `source`,
and nothing points back up. The lower two tiers' third-party sets don't intersect: `source` names
the network and never cpal, `playback` names cpal and never the network, so a new kind of source
is a new `AudioSource` and nothing else. A bare module name below belongs to the tier that owns it.

## Playback machine

- **Lock discipline is root `CLAUDE.md`'s.** What it costs here: the monitor decides under the
  `PlayerState` lock and acts after `exec_lock`, so the builder a decision lands in re-verifies it.
- **The queue is ids, and a transition starts from a record `PlayerState::window` already holds.**
  Nothing under the state lock may wait on the database, so the app's `tasks::queue_window` fills
  the window ahead and re-reads it on every library tick, which is how a retag reaches Now Playing.
  A transition onto a slot it lacks leaves the deck empty under `Loading` and owes
  `PlayerAction::StartWhenResolved`, handed to that task over `sinks.awaited_start`, the engine
  being unable to name the database. A crossfade never defers: a next with no record is no next.
- **Volume caps at unity** (`MAX_VOLUME`) and the mixer doesn't clamp, so lifting the ceiling owes
  clip protection. `state::volume_to_amplitude` is the one conversion; never re-roll `/ 100`.
- **Where the volume is applied is `AudioOutput::voice_gain`'s answer alone**: on the voices, or at
  unity under a claim carrying it on the device's control. Every reopen goes through
  `reopen_routed`, and a system move comes back through `ExternalVolume` and
  `tasks::device_volume`, the path an OS media panel's volume takes.
- **Transport builders ask `source_allows`, never `is_radio`** (`docs/adr/`), which answers only
  questions about a station itself.
- **Symphonia is one version, reached through `decode`.** `aac_trim` is codec-scoped and `mkv_trim`
  container-scoped, and neither may take the other's scope; both install in `file_decode`, never
  `decode`, a live mount having no container header and no gapless transition to spoil.
  `file_decode::probe_duration` is the store's one edge into the stack.
- **The sleep timer's end-of-track mode reaches in from `melodia-views`.**
  `PlayerState::pause_after_current_track` keeps the late preload and the crossfade off, so the
  track drains to `EndOfStream`, where `build_end_of_stream_actions` stops rather than advancing;
  `player_pause` can't run there, the lock not being reentrant. Armed after the preload has staged,
  it pauses at the next track's end.

## The DSP chain

Innermost first; everything up to the tap is one `EqSource`:

```
a file  → FileDecoder ──────────────────────────────┐
a station → StreamDecoder → ring → PrebufferSource ─┴→ EqSource:
    ReplayGain pre-gain → EQ bands → limiter → clamp → crossfade ramp → VisualizerTap
        → [deck: rate + channel conversion (speed lives here) → pause → volume] → mixer sum
            → dither, where a block the chain changed is narrowed to 16 or 24 bits
                → cpal's shared stream, or encode → an exclusive claim (ALSA hw:, WASAPI)
```

- **A reopen replaces the stream and reshapes the puller, and touches nothing above it**, because
  nothing loaded depends on the device: `Converter` takes the device shape per `fill`, and its
  state is the source's, handed on to a same-shape gapless successor. Decks, ramps, a staged
  source, the visualizer slots and the clock carry on, a mid-track reopen resuming on the same
  frame. **Never give a converter, a `Loaded` or anything built on the control thread a device
  field**, which is an append against the old shape. Pinned by `crates/melodia/tests/crossfade.rs`.
- **The ring is there for the network, not the decoder**: it keeps a blocking socket read off the
  audio callback thread, so a `FileDecoder` is pulled inline. A station shares everything from
  `EqSource` on, and gets no crossfade, gapless or ReplayGain, having no track boundaries or tags.
- **EQ and ReplayGain are orthogonal to `PlayerState`**: no `with_state_emit`, persisted through
  `EqualizerFlags` and `ReplayGainFlags`. A track's gain rides on `TrackSummary`, so the
  synchronous `play_track_inner` bakes it with no fetch, and a gapless stage takes it from the
  window's record into `preload_gapless`, never `PlayerAction::PreloadGapless`, which only clears.
- **`equalizer::PRESETS` order is load-bearing**: `EqPresetPicker` mirrors it in inline `@tr`
  names plus a trailing "Custom", and the persisted `CUSTOM_PRESET` is absent from `PRESETS`.
- **Nothing in the chain ends, starves or re-polls mid-frame**, half a frame flipping that deck's
  channel parity for everything after it: `EqSource::next` and `PrebufferSource` both decide at
  `frame_phase == 0`.

## Crossfade & decks

- **Two decks on one mixer, the ramp folded into `EqSource`** (`docs/adr/`). The complementary
  ramps keep the unclamped sum inside ±1 only while both advance together, which
  `output::mixer::LOCKSTEP_FRAMES` bounds; `crates/melodia/tests/crossfade.rs` derives its
  tolerance from that constant.
- **A deck's `clear()` blocks until the audio callback services it**, so a test pulling the mixer
  is that callback and has to run it on another thread.
- **`fade_ms` on `Pause` and `Stop` comes from the caller** (`library::playback`'s
  `transport_fade_ms`); everything the machine does for its own reasons passes `0`.

## Exclusive output and the verdict

- **One exclusive backend per platform behind one seam.** `output/mod.rs`'s `cfg_select!` binds
  `alsa`, `wasapi` or `unsupported` as `exclusive`, and `library::playback`'s `*_SUPPORTED`
  constants read those rather than spelling a platform list. Each compiles on its own platform only
  (`alsa-sys` probes pkg-config, which refuses a cross build), so a change to the seam is checked
  by CI's `clippy-windows` and `test-windows` or on the other machine, never assumed.
- **Samples become device bytes in one place, dithered first**: `encode`, reached only through
  `encode::BlockEncoder` (`crates/melodia/tests/bit_perfect.rs`). The shared `output_stream` calls
  `Dither::quantize` ahead of cpal's conversion; a narrowing without it plays correlated distortion
  at low levels and still passes every bit-perfect check.
- **A format change is decided at the boundary, and every path asks `plays_without_reopen`**:
  `reopen_for_track` before the append, `preload_gapless` refusing to stage, the monitor's
  `reopens_for` before a crossfade (the track drains to `EndOfStream` instead), and `start_track`
  again under the decks lock. A fade never reopens.
- **What a source is comes from the decoder, never the database.** `AudioSource::format` and
  `dsp_engaged` are required, a track start's request reads them before the append, and the verdict
  reads them back through `Voice::playing`. The `bit_depth` column feeds neither.
- **One saved device serves both modes where the backend's ids name a shared target**
  (`SHARED_DEVICE`): WASAPI's endpoint ids are the ones cpal opens by, where an ALSA `hw:` name
  would go round the sound server. `OutputChoice::shared_request` and `AudioOutput::fall_back` are
  the only places a shared open is handed a device.
- **A refused claim costs the claim, never the audio.** `AudioOutput::fall_back` opens shared and
  carries `Fallback { reason, device }` on `Negotiated` for the panel, the quality chip and the
  toast, whose Details action is `NotificationKind::ExclusiveRefused`, routed by
  `ui::shell::notifications::run_action`. A device that wasn't connected is looked for by
  `tasks::audio_health` through the lock-free `OutputStatus`, never the output's mutex, which a
  reopen holds.
- **A long pause can give the claim back, and play takes it again; no flag says so.** Play asks the
  deck (`holds_source()`), so any path leaving a paused track unseated is answered the same way.
- **At quit `PlaybackEngine::close_output` gives the card and its volume back**, `process::exit`
  running no destructor (`desktop-shell.md`'s Shutdown).
- **`signal_path::evaluate` decides every verdict, and the UI owns only its words.** A new stage
  that can change the samples is a field on `SignalInputs` and a row in `evaluate`, never a grade
  worked out in Slint; `SignalPathUi` words it once for the Output card and the quality chip.

## The live source and its reconnect

`stream_source`'s and `prebuffer`'s `//!` argue their designs; the facade, the off switch and the
station tables are `radio.md`'s.

- **The network never touches the audio callback thread** (`docs/adr/`), which is what
  `prebuffer`'s ring is for, and a dry ring yields `Some(0.0)`, never `None`: ending the source
  would tear the deck down on a stall the connection is about to recover from.
- **Reconnect lives in the feed thread, not the monitor** (`docs/adr/`). The monitor's
  end-of-stream arm sees only a station that has spent its budget, and stops rather than advancing:
  a station going off air must not start a library track.
- **A deck converts each source from the rate it reports, which only a second station shows
  wrong**, every sample below the mixer being correct. `crates/melodia/tests/stream_rate.rs` plays
  two at different rates through one deck and reads the rate off the mix.
- **For a station, `Loading` is the initial connect alone.** Every later gap stays `Playing` with
  `RadioNowPlaying.buffering` raised, since MPRIS reports `Loading` as Stopped.
- **Pausing a station drops its connection**: stream-download's bounded storage pauses the download
  when the reader falls behind, so a held-open socket resumes on stale audio.
  `library::playback::needs_station_reopen` routes the next play, shared with `player_play`.

## Visualizer — audio side

`visualizer.rs`, `spectrum.rs` and `waveform.rs` argue the design and every constant; the UI half
is `visualizer.md`.

- **The tap wraps `EqSource`, read-only and bit-identical**, so `EqSource`'s frame gate and bypass
  stay clear of it. It sits above the deck's conversion, so analyse against `analysis_rate()`,
  never `sample_rate()`; `PlaybackEngine::set_speed` is that cell's only writer.
- **Which rings are live is drop-scoped** (`DeckRun`), never a cursor delta, which reads a live
  deck as dead between cpal's period-sized pulls. A source's claims go on the audio thread through
  `AudioSource::release_claims` and only the free is queued, so a finished track leaves the window
  before it is collected (`crates/melodia/tests/crossfade.rs`).
- **`spectrum::NUM_BANDS` is the sole bar count**, crossing into Slint only as
  `Visualizer.band-count`, for the strip's width ceiling in `now-playing-view.slint`; a `.slint`
  literal in its place is the regression.
- **Retune against p90 per band**, never the mean, which every track with nothing in a band drags
  to zero: decode a couple of dozen real tracks through `SpectrumAnalyzer` at the real tick, in a
  throwaway harness.
