---
paths:
  - crates/melodia-audio/src/player/source/**/*.rs
  - crates/melodia/tests/**/*.rs
---

# Symphonia Best Practices

Decoding only, against 0.6.1.

## Opening and decoding

- **The probe never reads the `Hint`**: 0.6.1 scores each reader whose marker matches against the
  bytes after it, so a hint can't steer a wrong pick. `MediaSourceStream` is the read-ahead buffer,
  so nothing wraps the source in a `BufReader`.
- **`default_track` honours the container's default flag before the codec**, so a null track can
  win unless filtered.
- **`next_packet` answers `Ok(None)` at the end**, and a packet decodes to a
  `GenericAudioBufferRef`, flattened with `copy_to_vec_interleaved`.

## Seeking

- **`SeekMode::Accurate` with `SeekTo::Time`, then reset the decoder every time.** Upstream asks
  for it, and the reset is a no-op for PCM, ADPCM, ALAC and FLAC. MP3 rebuilds its state and AAC
  and Vorbis clear an overlap buffer that would otherwise blend the audio either side of the jump.
- **A demuxer seek lands on a packet boundary**, so the head is trimmed from
  `required_ts - actual_ts`. Nothing upstream does it.

## Gapless: what 0.6.1 leaves to us

`AudioDecoderOptions::gapless` defaults to `true`, and only MP3 and Vorbis act on it.

- **AAC in MP4**: isomp4 fills no packet trim and no `Track::delay`, and the AAC decoder ignores
  the flag.
- **Matroska**: `CodecDelay` already comes off the timestamps, and `DiscardPadding` is parsed and
  read nowhere.
- **Opus**: Symphonia ships no decoder.
- **`Track::delay` is a timeline offset, never frames to drop.** MP3 and CAF fill it and open their
  timeline at `-delay`, so theirs lines up; Ogg fills it for Opus from `pre_skip` and keeps
  `absgp_to_ts` at identity, so its timeline still counts the priming.
