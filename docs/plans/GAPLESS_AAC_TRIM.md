# Gapless AAC Trim

Working doc for [#91](https://github.com/KenanSalar/Melodia/issues/91). Delete when the feature
ships.

Status: **complete** · Created: 2026-09-02

> Upstream facts below were verified 2026-09-02 against the pinned `symphonia 0.6.1` sources in the
> registry, the checkout at `~/Development/Symphonia` (tag `v0.6.1`), and the rox and termusic
> checkouts beside it.

---

## What ships

An AAC album plays edge to edge: the encoder priming at the head of every track and the padding at
its tail stop reaching the deck, so a gapless record has no seam at a track boundary.

## Why nothing upstream does it

0.6 does gapless at the decoder, under `AudioDecoderOptions::gapless` (default on), and only the
MP3 and Vorbis decoders act on it.

- `symphonia-format-isomp4` parses the edit list into a private `ElstAtom` and never reads it back,
  fills neither `Track::delay` nor `Track::padding`, and builds every packet through
  `Packet::new`, which zeroes `trim_start`/`trim_end`.
- `symphonia-codec-aac` takes `_opts: &AudioDecoderOptions` and ignores it. There is no `.trim()`
  call in the crate.
- `iTunSMPB` is parsed nowhere in the workspace. An unmapped freeform key does survive as a raw tag
  (`add_mapped_tags` falls through to `add_tag`), so the string reaches `FormatReader::metadata()`
  under key `com.apple.iTunes:iTunSMPB`.

None of the three reference players fills the gap either: `symphonia-play` and termusic pass the
flag through and trust it, and rox documented a fallback it never wrote, shipping only a
`count_frames` harness to check the MP3 path.

## The numbers, and where they live

| source | written by | what it states |
|---|---|---|
| `iTunSMPB` freeform tag | iTunes, qaac, Apple Music | priming, remainder, original sample count |
| `moov/trak/edts/elst` | ffmpeg and most other muxers | where the presentation starts, and how long it runs |

`iTunSMPB` wins where both are present: it names the original sample count outright.

An edit list needs both of its numbers, and the second one is not interchangeable with the track
duration. Fraunhofer's own gapless test pair states `segment_duration` against a movie timescale
equal to the media one, where it is exact and excludes the trailing padding; `mdhd duration` there
is a whole number of 1024-frame packets and still carries it. Deriving the length as
`duration - media_time` left 106 frames of padding at precisely the boundary those files exist to
test. So the edit's own length wins where its timescale converts exactly, and the derived one is
both the fallback and the ceiling. The rule is divisibility, not equal timescales: ffmpeg's 1000
against 44100 media rounds every segment duration to the millisecond and most of those do not
divide, so the derived length answers for them; one that does divide still carries that rounding and
is taken anyway, capped at the derived length, because cutting a millisecond beats leaving a
packet's worth of padding.

Measured on this tree's own fixture, so the scale is not hypothetical. `tests/assets/silence.m4a`
declares `elst media_time = 1024` over `mdhd timescale 44100, duration 45124`, and its AAC stream
decodes to 46080 frames: 1024 frames of encoder silence at the head and 956 of padding at the
tail, around 23 ms and 22 ms. Both survived before this.

## Scope

AAC alone, gated on `CODEC_ID_AAC`. FLAC and ALAC are lossless and pad nothing, Vorbis is trimmed
upstream, and Opus has no decoder here until [#35](https://github.com/KenanSalar/Melodia/issues/35).

Both ends are trimmed. Head silence alone is half the seam.

## Where it lives

`src/player/decode.rs` does not change. Its contract is that `file_decode` and `stream_decode`
differ only in what they open, and an encoder delay is part of what a *file* is: a live mount has
no container header to state one and no gapless transition to spoil. So the feature is
`src/player/aac_trim.rs` plus the install and the seek in `file_decode`, and the stream path is
untouched by construction rather than by a `None` threaded through the shared open.

`aac_trim` is named for and shaped like `aac_config`, its neighbour: an AAC-specific fixup
answering a question the demuxer will not.

## Phases

- [x] **1. `aac_trim`.** The box walk, the `iTunSMPB` parse, and the resolution of the two into one
      answer in decoded frames.
- [x] **2. `file_decode`.** Install the trim at the open, count the tail down in `next`, offset the
      seek by the head, and report the trimmed length as the duration.
- [x] **3. Manual test.** Confirmed by ear on the Fraunhofer pair, and mechanically against the real
      chain: the mixer's output over a gapless transition is bit-identical to the two sources
      decoded separately and concatenated, on the AAC pair and on two 48 kHz MP3s from the library.
      Nothing is inserted, dropped or duplicated at the handover.
- [x] **4. Tests.** `src/player/tests/aac_trim_tests.rs` and additions to `file_decode_tests.rs`.
      Every one was checked by mutation: removing the head skip, the tail cap, the stated length's
      precedence, or its cap each turns exactly one of them red.

      Two of them exist because no committed fixture can state the case. `tests/assets/` has no file
      whose edit list disagrees with its track duration, so the precedence is pinned against
      `resolve` directly, spelling the Fraunhofer numbers out. And the priming is invisible to a
      frame count while the tail cap holds it, so it is pinned with an `iTunSMPB` that overstates
      the length, which removes the cap and leaves the packets minus the priming.
- [x] **5. Docs.** `src/player/CLAUDE.md`, `.claude/rules/symphonia.md` and the README.

Feature complete. Delete this file once it ships.
