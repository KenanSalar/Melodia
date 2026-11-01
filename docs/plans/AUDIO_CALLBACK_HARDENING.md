# Plan: Audio callback hardening (decode ahead, and nothing freed on the audio thread)

Status: **in progress** (Phases 1 to 3 built, hand test passed) · Owner: Kenan · Created: 2026-11-01 · Tracking: #155

> **Validated 2026-11-01** against `8dd32dc3` (`main`): direct reads of `output/voice.rs`,
> `output/mixer.rs`, `output/device.rs`, `output/alsa.rs`, `output/realtime.rs`,
> `source/decode.rs`, `source/file_decode.rs`, `source/prebuffer.rs`, `source/stream_source.rs`,
> `engine/backend/mod.rs`, `engine/handlers.rs` and `net/pacer.rs`. Checked in sources: std's
> `sync/mpmc/waker.rs` on the 1.97 toolchain (`SyncWaker::notify` locks only while a thread is
> registered as blocked) and symphonia-core 0.6.1's `packet.rs` (`Packet::data` is a
> `Box<[u8]>`, so every `next_packet` allocates). Guidance: Ross Bencina, "Real-time audio
> programming 101: time waits for nothing".

Working doc. Delete when every phase has shipped and passed Kenan's hand test.

## Context

**What you'd hear today.** When the drive holding a track is slow to answer (a network share, a USB
drive waking up, a disk busy with a library scan), the stall happens inside the device callback,
because a local file is read and decoded there. The whole output drops out for that moment, both
decks and a crossfade included, rather than just the track on the slow drive. In exclusive mode the
same stall lands on the real-time writer thread rtkit promoted. No issue reports a dropout, so this
is a latent hazard found in review, not a field bug.

**The review's claims, checked against the code:**

- **Decode, DSP and mixing are pulled through the output path.** True, and understated.
  `decode.rs:319` says so in its own comment. `next_packet` reads the file through
  `MediaSourceStream` (a `read(2)` per refill) and allocates a fresh `Box<[u8]>` per packet. ADR 14
  took the network off the callback; the disk was never moved.
- **A full retirement queue frees the source on the audio thread.** True (`voice.rs:518-521`). It
  is reached when more than `SPENT_SLOTS` sources retire on one voice inside one 500 ms poll tick
  (`handlers.rs:363`), such as a burst of skips. A latent sibling: `staged` reallocates on the
  callback if appends pile up behind a `Clear` that failed to send.
- **The callback uses std channels, risking lock or allocator delays.** Not a hazard.
  `sync_channel(n)` is the preallocated array flavour, both ends use only `try_*`, and std takes its
  waker lock only while a thread is blocked on the channel, which never happens here. `service()`
  also skips the channel entirely when `issued == serviced`. No change.
- **`pacer.rs` uses `tokio::sync::Mutex` but holds no guard across `.await`.** True. `acquire`
  drops its guard at the end of a block before `sleep`, and `stop_for` never awaits under it. The
  tree's tokio rule asks for a sync mutex here.
- **UI tests read source rather than drive the UI, and lists keep their whole id keyset.** True and
  deliberate (ADR 36; `paged_rows/mod.rs:1-6`, 8 bytes a track). Out of scope.

**What keeps working.** Gapless seams, crossfades, seeks, the instant response of EQ, preamp and
volume, the clock, bit-perfect output and radio all behave as today. The one change a user could
notice: a slow drive briefly silences only the deck reading it, and that deck's clock holds still
until audio flows again.

## Decisions settled here

1. **Only decode moves off the callback.** The real-time rule is no blocking I/O, no allocation or
   free, and no lock a lower-priority thread can hold. It is not "no work". EQ, ReplayGain, the
   crossfade ramp, the resampler, the sum and volume are bounded arithmetic and stay where they are.
   Moving them behind a buffer as well would delay every EQ, preamp and crossfade change by that
   buffer's depth, put a flush on every seek and skip, and reopen ADR 9's decks, ADR 10's chain and
   the bit-perfect path. Decode is the one stage that breaks the rule, so it is the one that moves.
2. **The ring sits below the `AudioSource` boundary the voice already pulls.** ADR 14's ring,
   extended from the socket to the disk: a feeder thread owns the `FileDecoder` and fills a
   per-source ring, and the voice pulls the ring through the same chain (`EqSource`,
   `VisualizerTap`, the converter) as today. The clock, the gapless handover, the crossfade and
   bit-perfect output never see the ring.
3. **A starved file deck holds rather than playing silence as music.** A station's ring pads with
   silence, which is harmless with no clock. A file padding with silence would advance its clock
   through the stall and push lyrics, the scrobble position, the saved resume point and the track
   end out of step. Instead the voice stops pulling that deck for the step. The mixer has already
   zeroed the block, so the deck is silent while its converter, clock, fade ramp and tap all stand
   still. A clock that counts only what was actually pulled needs no correction afterwards.
4. **Seeks keep their shape.** A seek already builds a new, positioned source on the control thread
   (`backend/mod.rs:522-536`). It now wraps that source in a new ring, so the reader never seeks in
   place and answers `try_seek` with `NotSupported`.
5. **No inline fallback.** If the feeder thread can't start, the track fails to start with the usual
   error toast. Falling back to inline decode would keep two paths alive to serve a machine that is
   out of threads.
6. **The retirement queue becomes impossible to fill, rather than larger.** A larger queue makes an
   overflow rarer and no shorter. A cap on the sources a voice holds, plus draining before every
   send, bounds what can be in the queue, and that bound sizes it.
7. **The ring is filled before it mounts.** `DecodeAhead::start` decodes on the caller's thread,
   already off the deck lock for the open and seek, until the ring is full or the file ends, and
   only then hands the decoder to a feeder (none when the file ended first). A mount or a seek
   never starts on a hold, so a logged hold is always a stall.
8. **The depth is a parameter of the engine.** `with_output` passes `decode_ahead::DEPTH`; a suite
   pulling faster than real time passes one deeper than its fixtures, which with (7) decode whole
   before they mount. Repeating a held step instead loses nothing only when every deck held:
   mid-crossfade the deck that played moves its ramp and the held one doesn't, which breaks
   `crossfade.rs`'s summed-amplitude check within a few steps.

## Prior art

The guidance this follows, and the mechanisms that shaped the decisions:

- **The real-time audio rule** (Bencina): the callback must not block on I/O, allocate, free, or
  take a lock a lower-priority thread can hold, because any of those has an unbounded worst case.
  Decision 1 draws the line exactly there.
- **A clock counted on the consumer side** only advances when frames are actually played, so an
  underrun leaves it still and a deep buffer never has to be subtracted. Decision 3 applies this per
  deck.
- **A feeder that naps for a fraction of what it has buffered** wakes rarely while full, and still
  refills well before the ring drains. A fixed short poll, which suits a socket, would wake every
  file feeder about 50 times a second, paused included.
- **Two seconds of decoded audio ahead** rides out a read stall of 1.4 to 1.9 s at normal speed,
  where half a second covered 0.28 to 0.40 s: a NAS or Wi-Fi share pausing, not only a busy disk's
  seek. fooyin and Strawberry buffer four seconds, rox half of one. Four would double the memory
  again for stalls of two to four seconds, and a drive waking from sleep outlasts any of them.
- **std's bounded channel** (`sync/mpmc/array.rs`, `waker.rs`) never locks when neither end blocks,
  which is why the command and retirement channels stay as they are.

## Structure

| File | Owns after the change |
|---|---|
| `crates/melodia-audio/src/player/source/sample_ring.rs` **(new)** | The SPSC ring: `FeedState` (finish and abandon), `ring()`, `RingWriter` (given its depth and park interval) and `RingReader` with the whole-frame gate (`pull() -> Pulled`) |
| `crates/melodia-audio/src/player/source/prebuffer.rs` | Radio policy only: `StreamShared` (titles, the buffering flag) and `PrebufferSource` over a `RingReader` |
| `crates/melodia-audio/src/player/source/decode_ahead.rs` **(new)** | `DecodeAhead`: fills the ring, spawns the `"decode-ahead"` feeder that owns the rest of the `FileDecoder`, and reads its ring as an `AudioSource` |
| `crates/melodia-audio/src/player/source/audio.rs` | `AudioSource::is_ready`, provided (`true`): whether the deck should pull now |
| `crates/melodia-playback/src/player/playback/{equalizer,visualizer}.rs` | Forward it, as they forward `release_claims` |
| `crates/melodia-playback/src/player/playback/output/voice.rs` | The source cap, drain before send, the hold, and `held_steps` |
| `crates/melodia-playback/src/player/playback/decks.rs` | `held_steps` per deck, by slot |
| `crates/melodia-engine/src/player/engine/backend/mod.rs` | `new` takes the depth; `DecodeAhead::start` at the three mount sites |
| `crates/melodia-engine/src/player/engine/handlers.rs` | `HoldWatch`: the poll tick logs a stall once per episode |
| `crates/melodia-net/src/services/net/pacer.rs` | A `std::sync::Mutex`; `stop_for` turns synchronous |

- **Duplication collapsed.** The ring, the whole-frame gate and the abandon and finish protocol
  lived in `prebuffer.rs`. `sample_ring.rs` gives radio and files one copy.
- **Three mount sites, one wrapper.** A `FileDecoder` reaches a deck through `start_track` (for
  `play_media` and `begin_crossfade`), `seek` and `preload_gapless`, and each passes it through
  `DecodeAhead::start` before `build_source`.
- **Sizes.** `voice.rs` 589, `backend/mod.rs` 645, `handlers.rs` 579, `prebuffer.rs` 238. All under
  800.
- **Thread name.** `"decode-ahead"` is 12 bytes, under Linux's 15. It is written as a literal after
  `.name(`, which `packaging.rs`'s walk reads.

## Phases

Each phase ends at the static gates and a build. An existing test that a phase breaks is fixed in
that phase, and so is a doc it makes stale. Kenan hand-tests the whole change once, at the start of
Phase 4, and its new tests wait for his go after that.

Three commits: Phases 1 and 2 together, both small and neither audible; Phase 3, its ring move
included; then Phase 4.

### Phase 1: the pacer on a sync mutex

Status: built, gates green.

1. `pacer.rs`: switch to `std::sync::Mutex`, since `melodia-net` doesn't carry `parking_lot`.
   Lock with `.lock().unwrap_or_else(PoisonError::into_inner)`, the tree's existing form
   (`queries/track/columns.rs:27`). That is sound here because `Inner` is two `Option<Instant>`s
   and any value is valid. `acquire` stays `async` because it sleeps, and its guard already ends
   before the `.await`. `stop_for` becomes a plain `fn`.
2. `lyrics_directory/lrclib.rs:322` drops its `.await`, and so do the calls in
   `tests/pacer_tests.rs`.

### Phase 2: the retirement queue can't fill

Status: built, gates green.

1. **`SOURCE_SLOTS`** (`= COMMAND_SLOTS`): the most sources one voice holds, counted by
   `VoiceShared::sources`. `send_counted` claims a slot with `sources.try_update(…)` and refuses at
   the cap on the control thread, with the existing "transport op was lost" warning. A voice in
   normal use holds at most three (the current source, one staged, a seek's replacement), so the
   cap is never met. It counts the playing source as well, so it refuses one op before the channel
   would; what keeps a seek burst off it is each seek landing before the next (Phase 4's fix).
   `staged` is preallocated to `SOURCE_SLOTS`, so `push_back` can't reallocate on the callback.
2. **Drain before every send**: `Voice::send` calls `collect_spent()` first. The poll-tick drain
   stays for natural track ends. `send_counted` claims before that drain, so every source the queue
   can hold was counted at its last drain, and the queue never holds more than `SOURCE_SLOTS`.
3. **The spent queue is sized by `SOURCE_SLOTS` itself**, `SPENT_SLOTS` folding into it now that
   the bound needs no slack. `VoicePull::retire` keeps its `try_send`, with a `debug_assert!` on
   `TrySendError::Full` pinning the invariant in tests. `Disconnected`, which is teardown, still
   drops.
4. Docs that moved with it: `COMMAND_SLOTS`, `collect_spent`, `retire`, and the `spent` field,
   whose "the one collector never contends it" stopped being true. The lost-op warning and its
   reason moved to one `warn_lost_op`, shared by the cap and the full channel.

### Phase 3: decode ahead of the callback

Status: built, gates green.

1. **The ring move:** `SampleRing`, the frame gate and the finish and abandon flags (`FeedState`,
   separate because a station's exists before its ring) move from `prebuffer.rs` into
   `sample_ring.rs`. `ring()` takes the depth and the park interval: radio passes `PREBUFFER_MS` and
   `FEED_PARK`, files `DEPTH` and a quarter of it. `StreamShared` delegates `finish`,
   `is_finished` and `is_abandoned`, so no radio caller changed, and `prebuffer_tests.rs` passes
   untouched but for one import.
2. **`decode_ahead.rs`:** `DecodeAhead::start(decoder, depth)` copies `shape`, `format`,
   `total_duration` and `dsp_engaged`, fills the ring (decision 7), and spawns the feeder,
   `for sample in decoder { if !writer.push(sample) { return } }` then `finish()`, which owns the
   decoder so the file is closed off the callback. `release_claims` and the reader's `Drop` set the
   abandon flag. `DEPTH` (2 s, raised from 500 ms after the hand test) and `LOW_WATER` (100 ms,
   clamped to the depth) are documented by role; the table below is what sizes `LOW_WATER`.
3. **`AudioSource::is_ready`**, answered by `DecodeAhead` as `finished || buffered >= low_water` and
   forwarded by `EqSource` and `VisualizerTap`.
4. **The hold in `VoicePull::render`:** a source that isn't ready is left for the step, and
   `VoiceShared::held_steps` takes one relaxed add.
5. **The engine:** `new` takes the depth (decision 8). `start_track` starts the `DecodeAhead` ahead
   of `bump_epoch` and returns its error, so a failure touches no deck; `seek` and
   `preload_gapless` wrap after their open. `build_source` stays generic.
6. **The poll tick:** `HoldWatch` reads `PlaybackEngine::held_steps` after `collect_spent` and logs
   `Deck {slot} held: …` once per episode, never the path.
7. **Suites that pull faster than real time:** `crossfade.rs` (6 s fixtures) and `reads_tests.rs`
   (pulls past its 1 s fixture) pass a depth deeper than their fixtures. `bit_perfect.rs` (under
   200 ms) and `handlers_tests.rs` (pulls 46 ms) fit `DEPTH` as they are. `stream_rate.rs` plays a
   station and `transport_holds.rs` is a Slint walk, so neither mounts a file.
8. Comments that named the callback as where a file decodes (`decode.rs`, `source/mod.rs`, `Loaded`)
   now say where it does. No doc file named it.

`LOW_WATER` covers one fill's worst pull: `resample::REACH` (256 source frames) of lookahead plus
one `LOCKSTEP_FRAMES` (64 device frames) step at `MAX_SPEED` (2.0).

| source / device | lookahead | step at 2× | needed |
|---|---|---|---|
| 44.1 / 44.1 kHz | 5.8 ms | 2.9 ms | 8.7 ms |
| 384 / 44.1 kHz | 0.7 ms | 2.9 ms | 3.6 ms |
| 8 / 48 kHz | 32 ms | 2.7 ms | 34.7 ms |
| 8 / 8 kHz (exclusive at the file's rate) | 32 ms | 16 ms | 48 ms |

The feeder naps `DEPTH / 4` when full; at 2× that drains 1 s of a 2 s ring, leaving 1 s, above
`LOW_WATER`, so a healthy feeder never lets a deck hold.

### Phase 4: hand test, then tests

Status: hand test passed; tests wait for Kenan's go.

1. **Hand test**, run 2026-11-01 on the dev data folder (51,012 tracks) against the debug build,
   with release builds of this branch and of `main` for the comparisons. Everything listed passed.
   EQ and preamp were not driven, having no route but clicks; they sit above the ring in the chain,
   as volume does.
   - lyrics: five tracks were looked up and stored, one answered from its tag, and twelve track
     changes 250 ms apart drew no refusal of their own; the one track LRCLIB refuses was refused
     alone
   - gapless seams, MP3 and AAC: no sample gained or lost, identical to `main` at matched gain
   - a manual crossfade fades cleanly, and volume moves within 100 ms on both builds
   - 25 skips back to back and an arrow-key run: no lost transport op
   - bit-perfect: the PCM2902 claimed at the file's S16 and 44.1 kHz, with no fallback, read from
     its `hw_params` rather than compared byte for byte
   - a radio station plays, pauses and resumes
   - a FUSE-mounted track frozen for 2.5 s mid-crossfade and for 3 s alone: only that deck went
     quiet, its fade and position held, and `Deck 1 held` was logged once per stall
   - RSS with the 2 s ring: anonymous memory within 1.0 MiB of `main`, idle and playing, CPU time
     equal, and peak within 6.9 MiB, set by the startup library load before any ring exists

   **Found and fixed in it.** Ten MPRIS `SetPosition` calls inside one device period lost ops on
   `main` and here, and playback ended on an older target than the last. A seek was the one
   transport op that sent without waiting for the callback, and each request ran as its own task,
   reaching `exec_lock` in any order. Now `PlaybackEngine::seek` lands its swap before returning
   (`PlaybackEngine::land`: on the callback, or on the spot where no stream pulls), and the newest
   request wins through `PlayerStateHandle`'s seek slot, recorded before the spawn and taken under
   `exec_lock`. A burst coalesces in the landing wait. Rerun: no lost op, every burst ending on its
   last target, and single, paused and arrow-key seeks landing as before.

   Everything below waits for Kenan's go.
2. **Tests:** added where the landed change earns them. `prebuffer_tests.rs`'s ring-level cases
   moving to `sample_ring_tests.rs` goes with it.
3. **Last:** delete this plan, after the root `CLAUDE.md` check that what it built is argued at its
   anchors.

## Cross-cutting

- **Gates every phase:** `cargo fmt --all`, then
  `cargo clippy --all-targets --locked --workspace -- -D warnings`, then
  `cargo test --locked --workspace`, then `cargo build -p melodia`. After that, the comment audit
  from `code-style.md`. A touched `cfg_select!` output module also gets bare
  `rustfmt --edition 2024`.
- **Memory** (Phase 3 only; Phases 1 and 2 are neutral):
  - New, bounded, and freed when the source retires: one ring per live file source, meaning the
    current one plus one staged during a gapless preload or a crossfade. Decoded f32 at `DEPTH` is
    706 KB at 44.1 kHz stereo, 1.5 MB at 96 kHz and 3.1 MB at 192 kHz.
  - The fill before a start or a seek grows with `DEPTH`: 1.2 to 3.6 ms at 2 s in a release build
    with a warm page cache, across MP3, AAC, FLAC 16/44.1 and 24/96, Opus and Vorbis.
  - One thread per live file source longer than `DEPTH`. Its stack is virtual, and only touched
    pages count.
  - Moved, not added: decoder buffers and per-packet allocations go from the audio thread's glibc
    arena to the feeder's, under the cap `prepare_process` sets.
  - Kenan measures idle and playing RSS before and after, with `/usr/bin/time -v` and
    `MELODIA_RSS_SAMPLE=1`. If idle grows by more than 20 MB, it goes to `heaptrack` before merging.
- **Unaffected:** no migration, no settings field, no `@tr` string, and no change to radio's
  behaviour.
- **Lock order:** the drain before a send takes only the voice's own `spent` lock, a leaf, under the
  `exec_lock → PlayerState → decks → output` order the control thread already holds. A seek's
  landing takes the output's lock under the decks and lets it go before waiting.

## Settled after the hand test

- **`DEPTH` is 2 s**, up from 500 ms (the prior-art bullet above argues it). Kenan's call.
- **A hold is only logged** (`HoldWatch`), with no indicator of the kind a station's buffering
  shows. Kenan's call.
