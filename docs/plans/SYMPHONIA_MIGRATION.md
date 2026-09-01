# Migrating off rodio, onto Symphonia and cpal

Working doc. Background and shape for replacing rodio entirely.

Status: **both phases done** · Issue:
[#79](https://github.com/KenanSalar/Melodia/issues/79) · Created: 2026-08-20

> **Kept for [#84](https://github.com/KenanSalar/Melodia/issues/84)**, not because anything below
> is still a plan. Both phases have landed and rodio is out of the manifest and the lock file, so
> everything here is a record of *why* rather than a description of what to do — which is exactly
> what the architecture-decision pass needs as a source. Delete it once #84 has taken what it wants.
>
> Read `src/player/CLAUDE.md` and `src/player/output/` for the tree as it actually is; where this
> doc and the code disagree, the code is right.
>
> **Phase 1 shipped first**, so the tree compiled one Symphonia while rodio was cut to `playback`.
> Everything below about *two* majors is history rather than description. Six things that work
> found which this doc had wrong or did not know:
>
> 1. **`Track::num_frames` is the wrong field for a duration** and `Track::duration` alone is not
>    enough either. Upstream's own note says to present `duration`; Matroska sets neither on the
>    track and puts the segment's on `MediaInfo`. termusic uses `num_frames` exclusively and would
>    read a `.mka` as 0:00. All three sources are needed.
> 2. **Neither reference player implements the frame-accurate seek trim.** `symphonia-play` says so
>    in its own comment ("This is a half-baked approach to seeking!") and termusic seeks `Coarse`
>    and skips whole packets. rodio's `refine_position` was the only frame-accurate one of the
>    three, and reproducing it was the real work of the seek.
> 3. **HE-AAC was not the only file that would have stopped playing.** 0.6's PCM decoder added a
>    width check that `symphonia-format-caf` contradicts, so every A-law `.caf` fails to build a
>    decoder where 0.5 played it. `decode::drop_companded_sample_width` is the answer, and the
>    fixture walk in `file_decode`'s tests is what found it.
> 4. **The feature list had to widen to the union** of what stations serve and what the library
>    ingests, which retires this doc's "a format nothing streams is only another way to guess
>    wrong" — true under 0.5's marker match, moot under 0.6's scoring.
> 5. **"Gapless trimming, which turns out to be free" below is half right.** The flag does default
>    on, but only the MP3 and Vorbis decoders act on it in 0.6.1, and `symphonia-format-isomp4`
>    fills in neither the packet trims nor `Track::delay`/`padding`, so an iTunes `.m4a` keeps its
>    encoder delay. Nothing regressed: 0.5 gated the same two demuxers, and MP3 improved, since 0.6
>    emits its trims unconditionally. Trimming AAC delay is a feature to write, not a switch, and
>    belongs in its own issue rather than in #89's acceptance.
> 6. **`refine_position` was not the only thing the seek had to keep.** rodio's decoder also puts
>    the consumer back on the channel it was part way through, because rodio's channel converter
>    holds its own phase across a seek and nothing resets it. Without that the stereo image swaps
>    for the rest of the track, on roughly every other seek, and only the mixer can see it —
>    `tests/crossfade.rs::seeking_never_swaps_the_stereo_image` is what pins it.

> Every upstream fact below was verified **2026-08-20** against the pinned sources in the
> cargo registry and this tree's `Cargo.lock`. Versions move. Re-check the appendix before
> acting on any of it.

---

## The short version

The end state is no rodio: Symphonia decodes, cpal outputs, and everything between them is
ours. Melodia already runs half of that today, because internet radio decodes on Symphonia
0.6 and hands rodio nothing but samples.

**That end state is now the tree.** What follows was written when it was not: at the time the
build carried **two major versions of Symphonia**, local files decoding through `rodio::Decoder`
(0.5.5) and internet radio through `player::stream_decode` (0.6.1). Nothing was broken by that and
the split was deliberate, but it is the debt this migration cleared.

**`rodio::Decoder` is not an alternative decoder.** Every rodio feature this tree enables is
a `symphonia-*` one, so rodio is a wrapper around the same library. Dropping it means owning
what the wrapper does, not swapping decoding engines. What is genuinely rodio's own, and so
genuinely has to be rebuilt, is the layer underneath: the mixer, the player queues, the
device stream, and the sample rate conversion between them.

---

## How we ended up with two

### The bug that forced it

Internet radio stations would connect and then never play. No error, no toast, no timeout.
The only outward sign was the log filling with `skipping junk at N bytes` and
`invalid mpeg audio header`, offsets climbing forever.

The cause is in Symphonia 0.5's format probe. `Probe::format` takes a `Hint` and discards
it (the parameter is literally `_hint`), resolving the container by matching a two byte
marker instead, with scoring left as a `TODO` in the source. The only ADTS marker 0.5
registers is `0xFFF1`. A station sending MPEG-2 ADTS opens with `0xFFF9`, which matches
nothing, so the search runs on into the audio payload and the first stray MP3 marker it
finds there hands an AAC stream to the MP3 demuxer. `MpaReader::try_new` then loops in
`read_mpeg_frame_strict` hunting for two consecutive similar frames that do not exist, and
never returns.

Europe 1 was the reproduction. Its stream is `ff f9` at byte zero with 2111 consecutive
valid ADTS frames across 720 KB, and the probe false matched `0xFFFF` at offset 8077.
Pointing 0.5's `AdtsReader` at it by hand would not have helped either: its resync loop is
`while sync != 0xfff1`, so MPEG-2 ADTS is unreadable by that version whatever you do.

Symphonia 0.6 fixes both halves. Its probe scores each candidate against the frames that
follow (`Scoreable`, 16 kB scoring depth, `MpaReader::score` returning `Unsupported` when no
second frame follows), and its ADTS reader accepts all four sync words through
`is_sync_word`, `(sync & 0xfff6) == 0xfff0`.

rodio 0.22.2 and rodio `main` both pin `symphonia ^0.5.5`, so 0.6 cannot be reached through
rodio at all. The stream path was moved onto 0.6 directly, and all 45 non-HLS stations on
the default Browse page now decode and produce audio.

### The bug that came with it

Worth recording next to the first, because it is the same class of fault and it took a
second round of testing to find.

After the decoder fix, the first station of a session played correctly and every station
after it played fast or slow until the app restarted. `PrebufferSource::current_span_len`
returned `None`, which the rodio queue passes straight through, which
`UniformSourceIterator::bootstrap` turns into an unbounded `Take`. One `SampleRateConverter`
gets built from whichever source reached the deck first, and because the inner iterator
never ends there is never a boundary at which it is rebuilt. A 44.1 kHz station following a
24 kHz one played at 1.84x. Fixed by naming a frame aligned span.

The lesson is not that rodio is bad. It is that the contract between our `Source` and
rodio's mixer has corners that are easy to get wrong, silent when wrong, and only visible on
the second thing you play. Owning the decoder would not have prevented this one, since the
ring sits below it either way.

---

## What the split cost, while it lasted

- **Nine extra crates** in the build. Compile time and binary size, no runtime cost worth
  measuring: this is code pages, not heap, so it does not move RSS.
- **Two probes with different behaviour.** The same question about the same bytes gets a
  different answer depending on whether they arrived from a file or a socket. An MPEG-2
  `.aac` file sitting in someone's library hits exactly the bug radio just escaped, because
  local files still go through 0.5.
- **Two feature lists that can drift.** Our 0.6 features and rodio's 0.5 features are
  separate decisions, and nothing checks that they agree about which containers we support.

---

## The rodio surface to replace

Twenty files name rodio, but most only touch `Source`, which the DSP wrappers implement. The
actual API surface is small, and worth listing because it is the whole scope:

| Used | Where | Replaced by |
|---|---|---|
| `Decoder` | `rodio_backend::decode_file`, `media::metadata` duration fallback | our Symphonia 0.6 decoder |
| `Source` | every DSP wrapper: EQ, ReplayGain, fade, visualizer tap, prebuffer | a local trait with the same four methods |
| `Player` | `decks.rs`, one per deck | our own per deck source slot |
| `Mixer` | `AppState::init`, `decks::connect` | an unclamped two deck sum |
| `MixerDeviceSink`, `DeviceSinkBuilder` | `AppState::init`, `open_sink_or_fallback` | a cpal stream we own |
| `rodio::cpal::StreamError` | `tasks::audio_health` | cpal named directly |

That last row is the reassuring one. cpal is already in the lock file and already named in
this tree through rodio's re-export, so depending on it directly is a manifest line rather
than a new dependency, and the device loss classification in `tasks::audio_health` keeps
working unchanged.

Every row landed where it says. In order: `player::file_decode`, `player::audio::AudioSource`,
`player::output::voice`, `player::output::mixer`, `player::output::device`, and a one-line import
change in `player::stream_health`. The local trait does have four methods, though not the four this
table meant — `current_span_len` turned out to exist only for rodio's rebuild policy and `try_seek`
moved from provided to required.

---

## What rodio does for us that we would have to own

Each of these is real work, and each is already written and tested upstream.

- **A frame accurate seek.** `Decoder::try_seek` resets the decoder, then calls
  `refine_position`, which computes `required_ts - actual_ts` and skips exactly that many
  samples. A demuxer seek lands on a packet boundary; without that trim, every seek replays
  the tail of what came before. This is also precisely what a CUE span needs at its head,
  so CUE support does not require the migration.
- **A bounded source.** `Source::take_duration` ends a source naturally after a given
  length, which is how a CUE span's end can reuse the ordinary end of stream path and keep
  gapless, crossfade and stop after working without knowing spans exist.
- **Gapless trimming, which turns out to be free.** `decode_file` passes
  `with_gapless(true)`, reaching Symphonia 0.5's `FormatOptions::enable_gapless`. In 0.6 that
  option **moved rather than disappeared**: it is `AudioDecoderOptions::gapless`, it defaults
  to `true`, and the decoder still trims delay and padding itself. `player::stream_decode`
  already gets it by taking `AudioDecoderOptions::default()`. Verify against real LAME and
  iTunes files anyway, since Symphonia's trimming has had gaps historically, but there is
  nothing to implement.
- **The `Source` surface.** `current_span_len`, `channels`, `sample_rate`, and
  `total_duration` computed from `time_base` and `n_frames`.

And below the decoder, the pieces that are rodio's own rather than Symphonia's:

- **The device stream.** A cpal output stream, plus `open_sink_or_fallback`'s behaviour,
  which exists because `open_stream` turns any config the device rejects into a boot with no
  audio at all. Owning this also retires the `Box::leak` on `MixerDeviceSink` in
  `AppState::init`, and with it the `process::exit(0)` rationale that leak forced.
- **The mixer.** Summing two decks, unclamped. That is what rodio does and what the
  complementary linear crossfade already assumes, so it must stay unclamped.
- **Sample rate conversion.** Currently `UniformSourceIterator` does this invisibly at mixer
  add time. `rubato` is the standard answer, is what rodio itself is moving to, and Symphonia's
  own reference player already ships the glue for it. Read the caveat about its `unsafe impl`
  adapters under prior art before copying that shape.
- **The playback clock.** `Player::get_pos()` becomes frames the output callback consumed.
  See the risks section; this is the subtle one.
- **Pause, volume and speed**, which are rodio wrappers around the source today.

## What stays exactly as it is

Worth being explicit, because it is most of the audio code and none of it is rodio's:

the EQ biquads, ReplayGain, the limiter, the crossfade curve and its two predicates, the
visualizer tap and its per deck rings, the spectrum analyser, the waveform trace, the queue,
the state machine, and every `PlayerAction`. These are shaped by rodio's `Source` trait but
not by rodio. Define a local trait with the same four methods and each DSP file changes one
`impl` line.

---

## What migrating would buy

- One Symphonia instead of two.
- 0.6's scoring probe for local files, so a mislabelled extension or an MPEG-2 `.aac` file
  resolves correctly instead of being handed to whichever demuxer matched first.
- **Control of the codec registry.** `symphonia::default::get_codecs` returns a fixed
  registry; building our own lets us register decoders that are not in it. Opus is the live
  case, and `docs/plans/OPUS_SUPPORT.md` has the detail. This one does not need the wider
  migration, only our own registry.
- **The frame clock.** Sample accurate scheduling rather than sample accurate seeking. CUE
  spans do not need it (see below), but bit perfect output does, and owning the stream makes
  `docs/plans/BIT_PERFECT.md` substantially simpler: findings 1 through 3 there are all about
  working around rodio's mixer, its fixed construction time rate, and the leaked sink.
- **No upstream release to wait on.** Today Opus, Symphonia 0.6 for local files, and any
  probe fix all arrive on rodio's schedule. Afterwards they arrive on ours.

---

## What it would cost

- **The decode loop, the seek, and the gapless trim** described above.
- **The HE-AAC fallback**, which has its own section below. It is one narrow container case
  with a contained answer, not the wall it first looked like.
- **`media::metadata`'s duration fallback** rides the same builder through
  `player::rodio_backend::probe_duration`, so it moves with the decode path.
- **The output half**: a cpal stream, the mixer, rate conversion, and the clock, all listed
  under the rodio surface above.
- **Risk concentration.** This lands under the most subtle and best tested part of the app,
  in a release that ships with an auto updater, and its failures are audible rather than
  loud. The ranked list is in "Risks, in order" below.

---

## The HE-AAC case

The one behaviour difference between the two versions that reaches a user. It is worth
writing out in full, because at first glance it reads as a hard blocker and on inspection it
is not.

### What the two versions do

Both parse the audio specific config identically. When the object type is 5 (SBR) or 29
(PS), they read the extension sample rate, then **re-read the object type to get the base
layer**, which is 2 (AAC-LC), and record the SBR information separately. After parsing, the
object type is `Lc` in both versions.

The difference is a single clause in the complexity gate:

```rust
// 0.5.5
if (m4ainfo.otype != M4AType::Lc) || (m4ainfo.channels > 2) || (m4ainfo.samples != 1024)

// 0.6.0
if asc.object_type != AudioObjectType::Lc
    || asc.sbr_present            // the only new term
    || channels.count() > 2
    || asc.samples != 1024
```

0.5 never consults `sbr_present`, so it passes the gate and decodes the AAC-LC core,
discarding SBR and PS. That is not full playback: SBR is what doubles the sample rate and PS
is what synthesises the stereo image, so an HE-AAC v2 stream comes out as a **half rate mono
core**. Probing HE-AAC v2 stations during the radio work produced exactly that, 1 channel at
24000 Hz.

So "plays today" means plays muffled, and in mono where the source was stereo.

### Why 0.6 refuses

Deliberately, not by oversight. Symphonia issue #415, "Additional HE-AAC detection in the
decoder", was opened in November 2025 and closed before 0.6 shipped. Upstream chose a loud
"unsupported" over silently handing back a degraded version of the track, which is the
defensible call.

### What it actually affects

Only containers that carry a real audio specific config: **MP4 and M4A**, plus Matroska with
AAC in codec private data. ADTS is untouched in both directions, because its config is
synthesised with `sbr_present` false. That is why internet radio is unaffected and why this
never showed up during the radio work.

HE-AAC is also uncommon in a music library. Store purchased AAC is AAC-LC at 256 kbps; HE-AAC
appears in low bitrate material such as podcasts, broadcast rips and older small encodes.

### Three ways out

1. **Synthesise a plain AAC-LC config.** The rejection keys entirely on `sbr_present`, which
   comes from the config we hand the decoder. Both versions read the core sampling frequency
   and channel configuration *before* the SBR branch, so those values are already exactly the
   core layer's. Rewriting `extra_data` as a plain AAC-LC config (object type 2, the core
   frequency index, the core channel configuration, then a three bit GASpecificConfig) is
   about two bytes, and the encoder for it is a small bit writer that can be tested offline
   against fixture bytes. No new dependency. This restores 0.5's behaviour exactly, which
   means it reinstates the degradation upstream just decided against, so it is a "nothing
   that plays today stops playing" fallback rather than a fix. Best near term answer.
2. **Wait for upstream.** Symphonia PR #473, "feat(aac): Support to HE-AAC and HE-AAC v2", is
   open, not a draft, and around 9,100 lines added. If it lands we get better than rodio
   gives us today: full band stereo instead of a half rate mono core. It has been idle for
   some months, so watch it rather than plan around it.
3. **`symphonia-adapter-fdk-aac`.** Full HE-AAC immediately. The adapter is
   `(MIT OR Apache-2.0) AND MPL-2.0`, but it links Fraunhofer's FDK AAC, whose licence
   carries patent terms that keep it out of several distribution main repositories. Given
   five package formats each owing licence text, this is the wrong trade here.

### rodio is not the answer

rodio has no AAC decoder of its own; every audio feature it exposes is a `symphonia-*`
passthrough. Its only role is pinning 0.5, which is why these files play at all. It is the
status quo rather than a fix, and if rodio resolves its own Symphonia 0.6 update this arrives
whether we migrated or not.

---

## Two phases, and why that order

> **Both landed, in this order.** Phase 1 as [#89](https://github.com/KenanSalar/Melodia/issues/89)
> (with the HE-AAC fallback ahead of it as [#88](https://github.com/KenanSalar/Melodia/issues/88)),
> Phase 2 as [#90](https://github.com/KenanSalar/Melodia/issues/90). The split held up: Phase 1
> shipped on its own and left a surface small enough to replace deliberately, and Phase 2 arrived as
> one variable in the part of the app with the least visible failure modes. **The A/B advice below
> is the one piece that was not followed** — keeping both backends compiling would have meant two
> copies of `decks.rs`, the file most likely to hold the bug, so the fallback was the previous
> commit and a rebuild instead.

`player::prebuffer`'s ring is what makes this two steps instead of one cutover. Radio already
decodes on Symphonia 0.6, fills a ring, and hands rodio nothing but `f32`.

**Phase 1: take the decoder.** Move local files onto Symphonia 0.6 behind the same ring
shape. `decode_file` is the only entry point, with four call sites plus `probe_duration`, so
the blast radius is one function. When this lands, every source reaching rodio is a ring
reader and rodio is reduced to summing two of them and pushing to a device.

**Phase 2: take the rest.** Replace that remnant with cpal directly, plus our own mixer,
clock, and rate conversion, and swap `rodio::Source` for a local trait.

The order is the point. Phase 1 is shippable on its own, collapses the two Symphonias, and
leaves rodio holding a surface small enough to reason about completely before replacing it.
Doing them together means changing the decoder and the output in one release, in the part of
the app with the least visible failure modes.

Each phase should keep the old path compiling alongside the new one long enough to A/B them,
because most of what can go wrong here is audible rather than a compile error.

## What Phase 2 found

The counterpart to the six corrections at the top of this file. Seven things the work settled that
this doc had wrong, or did not think to ask.

1. **No rubato, and therefore no `unsafe` question.** The prior-art section below calls
   `symphonia-play`'s `resampler.rs` "Phase 2's rate conversion, already written" and worries about
   its three `unsafe impl` blocks. Neither applies. rox — the one reference that owns its output —
   ships a hand-rolled linear resampler and defers rubato; the safe form of the symphonia-play glue
   needs planar scratch, a de-interleave pair and truncation of the silence its flush pads with. And
   we have a requirement neither reference has: speed is continuously variable over 0.25 to 2.0, so
   the ratio moves at runtime, which a pull-driven linear converter answers per sample with no block
   buffer. `output::convert` is that. The workspace `unsafe_code = "deny"` was never in tension.
2. **The `Box::leak` retires; `process::exit(0)` does not.** This doc says owning the stream retires
   the leak "and with it the `process::exit(0)` rationale that leak forced". Half right. `main()`
   names *four* threads that never exit and only one was the leaked sink's — souvlaki's MPRIS
   thread, accesskit's a11y thread and parked tokio workers are the others, so the call stays and
   only its comment changed.
3. **The clock needed no recalibration, because its meaning did not change.** Risk 1 below says the
   clock "has to come from what the output callback consumed, not from what the decoder produced".
   rodio's `get_pos` already was that: `track_position` counted frames the mixer pulled, and the
   mixer is pulled from inside the cpal data callback. Nothing keyed off position needed retuning.
   What *did* change is that speed moved below the frame counter, which deleted `compute_position`,
   `media_to_output_ms`, `seek_to_media` and the re-anchoring seek in `set_speed` outright.
4. **A whole-block deck opens a crossfade overshoot this doc never anticipated.** Rendering each
   deck's entire block in turn means a fade armed between two decks' renders leaves the outgoing one
   at full gain for that block while the incoming ramps up, so the unclamped sum passes unity by the
   period over the fade — five percent at a 50 ms period and the shortest crossfade allowed. rodio
   pulled one sample from every voice in turn and bounded it to a frame. `mixer::LOCKSTEP_FRAMES` is
   the answer, and it was a tightened test that found it, not review.
5. **The local trait needs four methods, not five.** `current_span_len` exists only for
   `UniformSourceIterator`'s rebuild policy; a deck that owns its source list knows the boundary
   exactly. It, `Cursor::span_len`, `prebuffer::span_samples` and two tests all went, and
   `tests/stream_rate.rs` still pins the property they protected.
6. **Retiring finished sources off the audio thread is not free — it breaks the visualizer.**
   `decks.rs` had long documented freeing a decoder's 64 KiB buffer under the arena lock as an
   accepted cost. It is more than accepted: the visualizer's per-deck liveness is scoped to the
   source's *drop*, so deferring it leaves a dead deck's ring mixing into the analysis window. Tried,
   caught by `tests/crossfade.rs`, reverted.
7. **A mixer that zeroes the block and adds into it loses the sign of zero.** `0.0 + -0.0` is `0.0`,
   so a lone deck at unity would not have been the passthrough `BIT_PERFECT.md`'s whole claim rests
   on. The first contributing deck writes and the rest add; volume moved into the deck so unity is a
   short circuit rather than a multiply by one. Also found by a test, not by reading the code.

## Prior art worth reading first

Three references, each authoritative for a different layer. A day reading them before writing
anything is a day well spent.

### symphonia-play, for the decode loop itself

`symphonia-play` lives in Symphonia's own repository, and the v0.6.1 tag is the version we
compile, so it is the author's own answer to how the 0.6 API is meant to be used. Where
termusic and rox are each one team's implementation, this is the reference one. It is also the
smallest: 550 lines of `main.rs` and 177 of `resampler.rs` are the whole of what matters here.

- **The decode loop, seek, and the head trim.** `play()` takes a seek position, seeks, and
  carries the returned `seek_ts` into the decode loop so frames before the target are decoded
  and discarded. That is the canonical shape for the trim rodio's `refine_position` does today.
- **Gapless, confirmed at the source.** It calls
  `AudioDecoderOptions::default().gapless(!no_gapless)`. The author's own player putting the
  flag there settles that 0.6 moved the option rather than dropping it.
- **`resampler.rs` is Phase 2's rate conversion, already written.** 177 lines wrapping
  `rubato::Fft<f32>` in `FixedSync::Input` mode, with `Adapter` and `AdapterMut` impls over
  Symphonia's `AudioBuffer` so rubato reads and writes those buffers in place.

  **One caveat that matters here specifically.** Those adapters are three `unsafe impl`
  blocks, because rubato's adapter traits are unsafe. This tree sets `unsafe_code = "deny"`
  at workspace level and `.claude/rules/unsafe-rust.md` limits the exception to platform FFI.
  So either copy through an interleaved buffer instead of adapting in place, or that rule gains
  a new sanctioned category. Decide which before adopting the shape, not after.

What it is **not** is a player. Zero mentions of a mixer, a queue, a playlist or a crossfade:
it decodes one file to one device and exits. Its Linux output is PulseAudio through
`libpulse-binding`, a C dependency we do not want; cpal, `rb` and `rubato` appear only on the
non-Linux target. So take the decode loop and the resampler from it, and the output layer from
somewhere else.

MPL-2.0, which is file level copyleft and compatible with this tree.

### termusic, for Phase 1

[**termusic**](https://github.com/tramhao/termusic) is the closer match, and it is proof that
Phase 1's end state is a real place to stand rather than a staging post we invented. Its
manifest is exactly what Phase 1 produces:

```toml
symphonia = { version = "0.6.0", features = [...] }
symphonia-adapter-libopus = { version = "0.3.0", default-features = false }
rodio = { version = "0.22", default-features = false, features = ["playback"] }
```

`playback` only, with **no `symphonia-*` features on rodio at all**. rodio is kept purely as
mixer and output; every byte is decoded by their own Symphonia 0.6 code. Their `rusty` backend
is around 7,500 lines total, but the part that matches Phase 1 is small:

| What | Where, relative to that repository |
|---|---|
| Own Symphonia 0.6 decoder implementing rodio's `Source` | `playback/src/backends/rusty/decoder/mod.rs`, 592 lines |
| The `MediaSource` shim | `playback/src/backends/rusty/decoder/read_seek_source.rs`, 39 lines |
| Their sink over rodio's playback half | `playback/src/backends/rusty/sink.rs`, 379 lines |
| A ring between decode and output | `playback/src/backends/rusty/source/async_ring/` |
| ICY metadata, comparable to our radio path | `playback/src/backends/rusty/icy_metadata.rs` |
| Pitch preserving speed, which we deliberately lack | `playback/src/backends/rusty/source/soundtouch/` |

Four things in that decoder are worth taking as findings rather than rediscovering:

- **`current_span_len` returns `Some(self.buffer.frame_len)`**, never `None`. They reached the
  same conclusion the span bug above forced on us, independently.
- **The audio track is chosen defensively.** `default_track(TrackType::Audio)` can return a
  track whose codec is null, so they filter it and fall back to the first non null one, citing
  Symphonia issue #258. `player::stream_decode` already does this through `names_a_codec`; the
  file decoder owes the same guard rather than trusting `default_track`.
- **The decoder is reset selectively after a seek**, only for MP3, citing Symphonia issue
  #274. **This one was followed and then walked back**: #274 reports MP3 seeks popping and
  says the fault is *not* seen with MKA/Vorbis or M4A/AAC, so it argues for resetting MP3 and
  says nothing about a reset harming anything else. In 0.6.1 AAC and Vorbis clear an overlap-add
  buffer on reset and FLAC/ALAC/PCM/ADPCM document theirs as no-ops, so the selective version
  costs the two codecs that need it most and saves nothing. `player::file_decode` resets
  unconditionally, as rodio and rox do.
- **They build their own `CodecRegistry`** in a `LazyLock`: `CodecRegistry::new()`,
  `register_enabled_codecs`, then `register_audio_decoder::<OpusDecoder>()` behind a feature.
  That is precisely the Opus route in `docs/plans/OPUS_SUPPORT.md`, working in production.

termusic is **MIT**, so unlike the project below, lifting from it with attribution is a real
option rather than a licence problem.

### rox, for Phase 2

[**rox**](https://github.com/zealsprince/rox) goes the whole way: cpal and Symphonia 0.6 with
no rodio at all. Where termusic stops at the decoder, rox owns the output too, which is what
Phase 2 needs. It also writes its decisions down rather than leaving them to be inferred:

| What | Where, relative to that repository |
|---|---|
| Why cpal and Symphonia rather than rodio | `docs/02-architecture/decisions/02-adr-audio-stack.md` |
| Single long lived stream, decoder swapped underneath at track boundaries | `03-adr-gapless.md` |
| Output structure, and the platform backends | `09-adr-audio-output.md`, `crates/rox-playback/src/output.rs` |
| The processing chain and exclusive mode | `19-adr-processing-chain.md` |
| Decode thread, ring, and the track boundary | `crates/rox-playback/src/engine.rs` |
| Rate conversion and clock accounting | `crates/rox-playback/src/resample.rs`, `latency.rs` |
| CUE spans, for later | `21-adr-cue-subsongs.md` |

Its gapless ADR carries a caution worth having in advance: it does not trust Symphonia's
delay and padding trimming, citing a known MP3 LAME header gap, and trims from tags itself
against real LAME and iTunes files. Symphonia 0.6 does trim by default, so take this as a
reason to verify rather than a reason to reimplement.

**Read it for the shape, then write our own.** Two reasons, and the less interesting one is
legal: rox is AGPL-3.0-only against this tree's AGPL-3.0-or-later, so lifting code would
narrow Melodia's licence and add attribution obligations across five package formats. The
better reason is structural. A player built this way from scratch folds queue, ordering,
crossfade and mixing into one engine type, because it had no existing player to fit around.
Melodia already owns all of those separately, and tested, so the boundaries land in different
places here. Its engine is the answer to a different question.

### And the one being replaced

Read rodio's own `decoder/symphonia.rs`, 335 lines plus a 69 line `read_seek_source.rs`. That
is precisely what Phase 1 replaces, so it doubles as the specification for what must not be
lost. Comparing it against termusic's 592 line equivalent is the cheapest possible estimate of
the work.

## Risks, in order

> **How the four actually landed**, since a risk list is only worth keeping if it says whether it
> was right. **1 was wrong about the mechanism and right about nothing needing to break** — rodio's
> position already counted frames pulled inside the cpal callback, so the quantity never changed and
> no consumer was recalibrated; what the rewrite bought was moving speed below the counter, which
> deleted the two-timeline machinery. **2 was right and was already paid** in Phase 1, where the
> unconditional reset, the head trim and the end margin all landed. **3 was right, and understated**:
> the mixer does stay unclamped, but the identity that makes that safe needs both ramps to advance
> together, which per-deck block rendering breaks — see item 4 of *What Phase 2 found*. **4 was
> right and cost one import line**, since pinning cpal to the version rodio already resolved made
> `rodio::cpal::StreamError` and `cpal::StreamError` the same type. The risk nobody listed is the one
> that bit: the visualizer's liveness being scoped to a source's drop.

1. **The playback clock.** Position, playback speed, crossfade timing, the sleep timer and
   gapless all key off `get_pos()` today. A ring puts distance between decoded and audible,
   so the clock has to come from what the output callback consumed, not from what the decoder
   produced. This is the change most likely to be subtly wrong and least likely to fail a
   test.
2. **Seek.** Reset the decoder after every one: the codecs that keep no state say so in their
   own `reset`, and AAC and Vorbis hold an overlap-add buffer that blends the audio either side
   of the jump if it is skipped. Pair the seek with the head trim that drops frames between the
   packet landing and the requested position, or every seek replays the tail before it, and
   clamp the target short of a stated length rather than onto it.
3. **Crossfade amplitude.** The mixer must stay unclamped, because the complementary linear
   ramps summing to unity is what keeps it from clipping. `tests/crossfade.rs` already pins
   this against a device free mixer and should keep passing across both phases.
4. **Device loss.** `tasks::audio_health` classifies on the rate of cpal `other` errors
   because ALSA folds everything into `BackendSpecific`. That logic is cpal level and
   survives, but it is currently reached through rodio's re-export.

---

## Sequencing

> **Spent.** Radio shipped, then the HE-AAC fallback (#88), then Phase 1 (#89), then Phase 2 (#90),
> exactly in that order. Of the three things that would have changed the plan, only the second and
> third are still open, and both are now easier than this section assumed.

Radio ships first. After that, Phase 1, then Phase 2, with the HE-AAC fallback written before
Phase 1 switches over so nothing that plays today stops playing.

Three things would change the plan rather than cancel it:

1. **rodio lands its own Symphonia 0.6 update.** ~~Phase 1 becomes a version bump and you skip
   to Phase 2.~~ Moot — it never landed in time and rodio is out of the tree either way.
2. **Symphonia PR #473 lands.** The HE-AAC fallback becomes unnecessary and HE-AAC gets
   better than it is today, full band stereo instead of a half rate mono core. Still open, and
   `aac_config_tests` says in its own header that the answer then is to delete the module.
3. **Opus.** It needs only our own codec registry, not the migration, so it can land on the
   radio path at any point without waiting for either phase. **This turned out to understate it**:
   `docs/plans/OPUS_SUPPORT.md` was written around rodio 0.23 registering the adapter behind a
   feature flag, and with the registry ours the remaining work is four lines. That doc's header
   now says so.

Two features already documented elsewhere get easier once Phase 2 is done rather than
requiring it: `BIT_PERFECT.md`, whose first three findings are all rodio workarounds, and CUE
spans, which need a frame accurate seek and a bounded source. Both of those exist in rodio
today, so **CUE does not depend on this migration** and should not be sequenced behind it.

Both of those held. `BIT_PERFECT.md`'s findings 1 through 3 and most of its Phase 1 are spent, and
each is marked there. CUE still does not depend on this: the frame-accurate seek is
`file_decode::try_seek` now rather than rodio's `refine_position`, and the bounded source it wants
is a span the deck can end — neither needed the output rewrite.

---

## Appendix: verified references

| Claim | Source |
|---|---|
| rodio wraps Symphonia rather than replacing it | `Cargo.toml` rodio feature list is entirely `symphonia-*` |
| rodio pins Symphonia 0.5 | `rodio-0.22.2/Cargo.toml`, and rodio `main` `Cargo.toml:157`, both `symphonia = "0.5.5"` |
| 0.5's probe discards the hint | `symphonia-core-0.5.5/src/probe.rs`, `pub fn format(&self, _hint: &Hint, …)`, with `// TODO: Implement scoring.` in `next` |
| 0.5 registers one ADTS marker | `symphonia-codec-aac-0.5.5/src/adts.rs`, `&[&[0xff, 0xf1]]` |
| 0.5 cannot resync MPEG-2 ADTS | same file, `AdtsHeader::sync`, `while sync != 0xfff1` |
| The MP3 demuxer loops on a false match | `symphonia-bundle-mp3-0.5.5/src/demuxer.rs`, `read_mpeg_frame_strict` |
| 0.6 scores candidates | `symphonia-core-0.6.0/src/formats/probe.rs`, `Scoreable`, `max_score_depth: 16 * 1024` |
| 0.6 accepts all four ADTS sync words | `symphonia-codec-aac-0.6.0/src/adts.rs`, `is_sync_word`, `(sync & 0xfff6) == 0xfff0` |
| 0.6 refuses SBR through an ASC | `symphonia-codec-aac-0.6.0/src/aac/mod.rs`, `asc.sbr_present` guard returning `"aac too complex"` |
| 0.5 has the same gate without that term | `symphonia-codec-aac-0.5.5/src/aac/mod.rs`, `if (m4ainfo.otype != M4AType::Lc) \|\| (m4ainfo.channels > 2) \|\| (m4ainfo.samples != 1024)` |
| Both unwrap SBR to the AAC-LC base layer | `symphonia-codec-aac-0.5.5/src/aac/mod.rs`, `M4AInfo::read`, the `if (self.otype == M4AType::Sbr) \|\| (self.otype == M4AType::PS)` branch re-reading the object type |
| The core rate and channels are read before that branch | same function, `read_sampling_frequency` and `read_channel_config` precede it, which is what makes a synthesised AAC-LC config exact |
| ADTS synthesises a config with SBR false | `symphonia-codec-aac-0.6.0/src/aac/mod.rs`, the `else` branch when `params.extra_data` is `None` |
| 0.6's refusal is deliberate | [Symphonia issue #415](https://github.com/pdeljanov/Symphonia/issues/415), closed before 0.6 shipped |
| Real HE-AAC support is in flight | [Symphonia PR #473](https://github.com/pdeljanov/Symphonia/pull/473), open, not a draft, roughly +9,100 lines |
| The fdk-aac adapter's licence | `symphonia-adapter-fdk-aac` is `(MIT OR Apache-2.0) AND MPL-2.0`, over Fraunhofer's FDK AAC |
| rodio has no AAC of its own | its audio features are all `symphonia-*` passthroughs |
| rodio's own 0.6 update is unscheduled | [rodio issue #919](https://github.com/RustAudio/rodio/issues/919), open and unassigned, tied to the engine rewrite in [#901](https://github.com/RustAudio/rodio/issues/901) |
| 0.6 moved gapless, it did not drop it | `symphonia-core-0.6.0/src/codecs/audio.rs`, `AudioDecoderOptions::gapless`, default `true`, "the decoder will trim any delay or padding frames". `FormatOptions` no longer carries it |
| rodio's seek is frame accurate | `rodio-0.22.2/src/decoder/symphonia.rs`, `decoder.reset()` then `refine_position`, which skips `required_ts - actual_ts` |
| A bounded source exists | `rodio-0.22.2/src/source/mod.rs`, `Source::take_duration` |
| An unbounded span pins the resampler | `rodio-0.22.2/src/source/uniform.rs`, `bootstrap` builds `Take { n: span_len }` and `next` only rebuilds when the inner iterator ends |
| Symphonia has no native Opus | `symphonia` 0.6.1 `all-codecs` lists no `opus`; `symphonia-codec-opus/src/lib.rs` is a stub |
| The rodio surface is small | `grep -rhoE "rodio::[a-zA-Z_:]+" src` returns `Decoder`, `Source`, `mixer::Mixer`, `MixerDeviceSink`, `DeviceSinkBuilder`, `source::SeekError`, `cpal::StreamError` and nothing else |
| cpal is already a transitive dependency | pulled by rodio, and already named in this tree through `rodio::cpal` in `tasks::audio_health` |
| rodio is adopting rubato | [rodio issue #901](https://github.com/RustAudio/rodio/issues/901), "integrating the new rubato resampler", roughly 7,700 lines |
| The mixer sums without clamping | `rodio-0.22.2/src/mixer.rs`; the crossfade's complementary linear ramps depend on it |
| The decoder to port is one file | `rodio-0.22.2/src/decoder/symphonia.rs`, 335 lines, plus `read_seek_source.rs`, 69 |
| rodio is permissively licensed | `rodio-0.22.2/Cargo.toml`, `license = "MIT OR Apache-2.0"`, against this tree's AGPL-3.0-or-later |
| rox runs cpal plus Symphonia 0.6 with no rodio | its `crates/rox-playback/Cargo.toml` names `cpal` and `symphonia = "0.6"` and no rodio |
| rox is AGPL-3.0-only | its root `Cargo.toml`, `license = "AGPL-3.0-only"`, which is why it is read rather than lifted |
| termusic keeps rodio for output only | its root `Cargo.toml`, `rodio = { version = "0.22", default-features = false, features = ["playback"] }` beside `symphonia = "0.6.0"` |
| termusic is MIT | its root `Cargo.toml`, `license = "MIT"` |
| Their decoder names a finite span | `playback/src/backends/rusty/decoder/mod.rs`, `current_span_len` returns `Some(self.buffer.frame_len)` |
| `default_track` can return a null codec | same file, filtered with `is_codec_null` and a fallback, citing [Symphonia issue #258](https://github.com/pdeljanov/Symphonia/issues/258) |
| A working player resets selectively after a seek (not followed) | same file, reset only for `CODEC_ID_MP3`, citing [Symphonia issue #274](https://github.com/pdeljanov/Symphonia/issues/274), which reports MP3 popping and rules out MKA/Vorbis and M4A/AAC |
| Only MP3, AAC and Vorbis keep state a reset clears | `symphonia-bundle-mp3/src/decoder.rs`, `symphonia-codec-aac/src/aac/ics/mod.rs`, `symphonia-codec-vorbis/src/dsp.rs`; FLAC's and ALAC's `reset` say "nothing to do" |
| Seeking onto a stated length parks the reader at the end | rox, `crates/rox-playback/src/engine.rs`, `SEEK_END_MARGIN_SECS` and `inside_track` |
| A custom registry is how Opus gets registered | same file, `static CODEC_REGISTRY: LazyLock<CodecRegistry>` with `register_enabled_codecs` then `register_audio_decoder::<OpusDecoder>()` |
| Symphonia ships its own reference player | `symphonia-play/` in [the Symphonia repository](https://github.com/pdeljanov/Symphonia), MPL-2.0, at the v0.6.1 tag |
| Gapless lives on the decoder options, from the author | `symphonia-play/src/main.rs`, `AudioDecoderOptions::default().gapless(!args.get_flag("no-gapless"))` |
| The seek head trim is carried as a target timestamp | `symphonia-play/src/main.rs`, `play()` seeks then threads `seek_ts` into the decode loop to discard frames before it |
| The rubato glue already exists | `symphonia-play/src/resampler.rs`, 177 lines wrapping `rubato::Fft<f32>` in `FixedSync::Input` with `Adapter`/`AdapterMut` over `AudioBuffer` |
| That glue needs `unsafe` | same file, three `unsafe impl` blocks, against this tree's workspace level `unsafe_code = "deny"` |
| Its Linux output is not a model for us | `symphonia-play/Cargo.toml`, `libpulse-binding` and `libpulse-simple-binding` on Linux; `cpal`, `rb` and `rubato` only on non-Linux |
| It is a single file player | `grep -ciE "mixer\|crossfade\|playlist\|queue"` over its `main.rs` and `output.rs` returns zero |
