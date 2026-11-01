# Plan: Audio callback hardening (decode ahead, and nothing freed on the audio thread)

Status: **planned** (no phase started) · Owner: Kenan · Created: 2026-11-01 · Tracking: #155

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
- **About half a second of decoded audio ahead** covers a disk seek under load and a brief pause on
  a network share. It cannot cover a drive spinning up from sleep, which takes seconds; no
  reasonable buffer can.
- **std's bounded channel** (`sync/mpmc/array.rs`, `waker.rs`) never locks when neither end blocks,
  which is why the command and retirement channels stay as they are.

## Structure

| File | Owns after the change |
|---|---|
| `crates/melodia-audio/src/player/source/sample_ring.rs` **(new)** | The SPSC ring: `SampleRing`, `RingWriter` (given its park interval and the abandon and finish flags) and `RingReader` with the whole-frame starvation gate |
| `crates/melodia-audio/src/player/source/prebuffer.rs` | Radio policy only: `StreamShared` (titles, the buffering flag) and `PrebufferSource` over a `RingReader` |
| `crates/melodia-audio/src/player/source/decode_ahead.rs` **(new)** | `DecodeAhead`: spawns the `"decode-ahead"` feeder that owns a `FileDecoder`, and reads its ring as an `AudioSource` |
| `crates/melodia-audio/src/player/source/audio.rs` | `AudioSource` gains one provided method: whether a pull now finds audio already decoded (`true` by default) |
| `crates/melodia-playback/src/player/playback/{equalizer,visualizer}.rs` | Forward that method, as they already forward `total_duration` and `release_claims` |
| `crates/melodia-playback/src/player/playback/output/voice.rs` | The source cap, drain before send, the hold, and the `holds` counter |
| `crates/melodia-engine/src/player/engine/backend/mod.rs` | One helper wrapping a `FileDecoder` in `DecodeAhead` before `build_source`, used at all three mount sites |
| `crates/melodia-engine/src/player/engine/handlers.rs` | The poll tick reads `holds` and logs once per episode |
| `crates/melodia-net/src/services/net/pacer.rs` | A `std::sync::Mutex`; `stop_for` turns synchronous |

- **Duplication collapsed.** The ring, the whole-frame gate and the abandon and finish protocol live
  in `prebuffer.rs` today. Lifting them into `sample_ring.rs` gives radio and files one copy, and is
  its own no-behaviour-change step at the start of Phase 3.
- **One wrapping site.** `FileDecoder` reaches a deck at `start_track` (through `play_media`), the
  seek at `backend/mod.rs:522-536`, and the gapless stage at `:589`. All three go through one helper,
  so no mount can skip the ring.
- **Sizes.** `voice.rs` is at 563 lines and gains about 30. `prebuffer.rs` shrinks.
  `backend/mod.rs` is at 625 and gains about 15. Everything stays well under 800.
- **Thread name.** `"decode-ahead"` is 12 bytes, under Linux's 15. It is written as a literal after
  `.name(`, which `packaging.rs`'s walk reads.

## Phases

Each phase ends at the static gates and a build, and Kenan runs it. An existing test that a phase
breaks is fixed in that phase. New tests and doc files wait for Phase 4's go.

### Phase 1: the pacer on a sync mutex

Status: not started.

1. `pacer.rs`: switch to `std::sync::Mutex`, since `melodia-net` doesn't carry `parking_lot`.
   Lock with `.lock().unwrap_or_else(PoisonError::into_inner)`, the tree's existing form
   (`queries/track/columns.rs:27`). That is sound here because `Inner` is two `Option<Instant>`s
   and any value is valid. `acquire` stays `async` because it sleeps, and its guard already ends
   before the `.await`. `stop_for` becomes a plain `fn`.
2. `lyrics_directory/lrclib.rs:322` drops its `.await`, and so do the calls in
   `tests/pacer_tests.rs`.

**Hand test:** lyrics still load for a track that has none cached, and a burst of track changes
doesn't trip the host's rate limit.

### Phase 2: the retirement queue can't fill

Status: not started.

1. **`SOURCE_SLOTS`**: the most sources one voice holds, counted by `VoiceShared::sources`.
   `send_counted` claims a slot with `sources.try_update(…)` and refuses at the cap on the control
   thread, with the existing "transport op was lost" warning. A voice in normal use holds the
   current source plus at most one staged, so the cap is never met. `staged` is preallocated to
   `SOURCE_SLOTS`, so `push_back` can't reallocate on the callback.
2. **Drain before every send**: `Voice::send` calls `collect_spent()` first. The poll-tick drain
   stays for natural track ends. Each source in the queue was alive at the last drain or was the one
   sent straight after it, so the queue never holds more than `SOURCE_SLOTS + 1`.
3. **`SPENT_SLOTS = SOURCE_SLOTS + 1`**, with a `const` assertion beside it. `VoicePull::retire`
   keeps its `try_send`, with a `debug_assert!` on `TrySendError::Full` pinning the invariant in
   tests. `Disconnected`, which is teardown, still drops.
4. Docs that move with it: `SPENT_SLOTS`, `COMMAND_SLOTS`, `Loaded`, `collect_spent`, and the
   `spent` field, whose "the one collector never contends it" stops being true.

**Hand test:** skip quickly through a long queue, and seek repeatedly by keyboard. Playback carries
on, and the log shows no lost transport op.

### Phase 3: decode ahead of the callback

Status: not started.

1. **Pure move:** `SampleRing`, `RingWriter` and the frame gate move from `prebuffer.rs` into
   `sample_ring.rs`, with `PrebufferSource` rebuilt on `RingReader`. Radio behaves as before, and
   `prebuffer_tests.rs` moves with what it pins.
2. **`RingWriter` takes its park interval.** Radio keeps `FEED_PARK` (20 ms). Files park for a
   quarter of their ring.
3. **`decode_ahead.rs`:**
   - `DecodeAhead::spawn(decoder: FileDecoder) -> Result<Self, AppError>` sizes the ring from the
     decoder's shape, starts the feeder, and copies `shape`, `format`, `total_duration` and
     `dsp_engaged`.
   - The feeder loop is `for sample in decoder { if !writer.push(sample) { return } }`, then
     `finish()`. The thread owns the decoder, so the file is closed and the decoder freed off the
     callback.
   - `release_claims` and `Drop` set the abandon flag, the same protocol radio uses.
   - Constants, each documented by its role rather than its value: `AHEAD_MS` (500) for ring depth,
     and `LOW_WATER_MS` for the least a deck must hold before it is pulled. `LOW_WATER_MS` has to
     exceed one `LOCKSTEP_FRAMES` step's pull at the top speed plus the sinc kernel's lookahead.
4. **The readiness method** on `AudioSource`. `DecodeAhead` answers
   `finished || ring.len() >= low_water`, and `EqSource` and `VisualizerTap` forward it.
5. **The hold in `VoicePull::render`** (`voice.rs:362`): when the current source isn't ready, it
   stops for this step and writes nothing. `VoiceShared::holds` takes one relaxed add per held step.
   This also covers the few milliseconds after a mount or a seek before the first fill lands. A
   staged gapless source is built seconds before its seam, so its ring is full by the handover.
6. **The engine:** one helper wraps the `FileDecoder` and then calls `build_source`, at the three
   mount sites. `play_media` already opens and seeks off the deck lock, so the spawn happens there.
   `plays_without_reopen` reads shape and format from `DecodeAhead`.
7. **The poll tick** reads `holds` and logs once per episode, naming the deck and never the path.
8. **Corpus walks that drive the engine faster than real time** (`bit_perfect.rs`, `crossfade.rs`,
   `stream_rate.rs`, `transport_holds.rs`) can outrun a feeder, and a held step would break
   `bit_perfect.rs`'s whole-PCM search. Their harness pulls one `LOCKSTEP_FRAMES` step at a time
   and repeats a step whose pull moved `holds`. A held step consumed nothing, so dropping its output
   loses nothing. Fixtures shorter than `AHEAD_MS` can instead wait, within their existing
   `START_BUDGET`, for the feeder to finish.
9. `source/mod.rs`'s doc: the ring keeps every blocking read off the callback, socket and disk
   alike.

**Hand test** (`cargo run -p melodia`, dev data folder):
- local playback, a gapless album seam (AAC and MP3), and a manual crossfade
- seeks, and a burst of skips
- EQ, preamp and volume still respond instantly
- bit-perfect playback on an ALSA `hw:` device
- a radio station
- a track on a USB drive or network share, interrupted for a moment: only that deck goes quiet, its
  position holds still, and the log shows the hold
- RSS before and after (see Cross-cutting)

### Phase 4: tests and docs (each waits for Kenan's go)

1. **New pins:**
   - The retirement queue never fills under a burst of clears and replaces.
   - A held deck leaves its clock, converter and fade ramp still, and the other deck plays on.
   - `DecodeAhead` reports its end only once the feeder has finished and the ring has drained.
   - The feeder stops once the source is dropped.
   - Each pin is shown to fail by breaking its code on purpose.
2. **Docs:** the `.claude/rules/` and `README.md` entries this makes stale, if any.
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
    current one plus one staged during a gapless preload or a crossfade. Decoded f32 at `AHEAD_MS`
    is 176 KB at 44.1 kHz stereo, 384 KB at 96 kHz and 768 KB at 192 kHz.
  - One thread per live file source. Its stack is virtual, and only touched pages count.
  - Moved, not added: decoder buffers and per-packet allocations go from the audio thread's glibc
    arena to the feeder's, under the cap `prepare_process` sets.
  - Kenan measures idle and playing RSS before and after, with `/usr/bin/time -v` and
    `MELODIA_RSS_SAMPLE=1`. If idle grows by more than 20 MB, it goes to `heaptrack` before merging.
- **Unaffected:** no migration, no settings field, no `@tr` string, and no change to radio's
  behaviour.
- **Lock order:** the drain before a send takes only the voice's own `spent` lock, a leaf, under the
  `exec_lock → PlayerState → decks → output` order the control thread already holds.

## Open questions

- Whether `AHEAD_MS` at 500 is right comes from the hand test on a slow drive. It is the one knob,
  and memory is the cost of raising it.
- Whether a hold is worth showing the user, as the buffering indicator does for a station, or only
  logging. This plan logs, and the hand test decides.
