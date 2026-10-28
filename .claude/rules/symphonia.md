---
paths:
  - crates/melodia-audio/src/player/source/**/*.rs
  - crates/melodia/tests/**/*.rs
---

# Symphonia Best Practices

Decoding only, against 0.6.1 through `player::source::decode`, the one Symphonia in the tree
(`docs/adr/`). What spans it and the DSP chain above it is `.claude/rules/audio-stack.md`'s.

## Opening and decoding

- **Open through `decode::open`, never a second probe.** 0.6.1's probe scores each reader whose
  marker matches against the bytes after it and never reads the `Hint`, so a hint can't steer a
  wrong pick; pass one anyway, the parameter staying. `MediaSourceStream` is the read-ahead
  buffer, so nothing wraps the source in a `BufReader`.
- **Pick the track through `decode::audio_track`**: `default_track` honours the container's default
  flag before the codec, so a null track can win unless filtered.
- **`decode::fill` is the one decode loop.** `next_packet` answers `Ok(None)` at the end, a
  `DecodeError` packet is skipped (a mount joined mid-frame opens with a few), and `ResetRequired`
  ends the source. A packet decodes to a `GenericAudioBufferRef`, flattened with
  `copy_to_vec_interleaved`.

## Seeking

`file_decode::try_seek` holds all three, and `file_decode_tests.rs` catches a missing trim.

- **`SeekMode::Accurate` with `SeekTo::Time`, then reset the decoder every time.** Upstream asks
  for it, and the reset is a no-op for PCM, ADPCM, ALAC and FLAC. MP3 rebuilds its state and AAC
  and Vorbis clear an overlap buffer that would otherwise blend the audio either side of the jump.
- **A demuxer seek lands on a packet boundary**, so the head is trimmed from
  `required_ts - actual_ts`. Nothing upstream does it.
- **Clamp short of a stated length**, never onto it (`SEEK_END_MARGIN`).

## Gapless: what 0.6.1 leaves to us

`AudioDecoderOptions::gapless` defaults to `true`, and only MP3 and Vorbis act on it. The rest of
the padding a file states is read here, `docs/adr/` arguing why we read it rather than trust the
flag, and `audio-stack.md` why both trims install in `file_decode` rather than `decode`.

- **AAC in MP4: `aac_trim`.** isomp4 fills no packet trim and no `Track::delay`, and the AAC
  decoder ignores the flag.
- **Matroska: `mkv_trim`, the tail for any codec.** `CodecDelay` already comes off the
  timestamps, and `DiscardPadding` is parsed and read nowhere.
- **Opus: `opus`**, which drops the priming off the identification packet in every container,
  Symphonia shipping no decoder.
- **`Track::delay` is a timeline offset by then, never frames to drop.** `decode::open` drops the
  `Track`, so no decoder sees it. MP3 and CAF fill it and open their timeline at `-delay`, so theirs
  lines up; Ogg fills it for Opus from `pre_skip` and keeps `absgp_to_ts` at identity, so its
  timeline still counts the priming `opus` dropped. Hence `file_decode::opus_priming`, gated on the
  codec rather than on the field.
