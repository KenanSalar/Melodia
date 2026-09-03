---
paths:
  - crates/melodia-audio/src/player/source/**/*.rs
  - crates/melodia/tests/**/*.rs
---

# Symphonia Best Practices

Decoding only. The layer under it — the device stream, the two decks, the rate conversion and the
clock — is `crates/melodia-playback/src/player/playback/output/`, which is a directory with its
own anchor files and argues itself
there; `.claude/rules/audio-stack.md` holds what spans it and the DSP chain above it.

## Symphonia — Audio Decoding

### Format Probing

- **A `Hint` does not steer 0.5's probe** — `Probe::format` takes it as `_hint` and resolves the format by matching a two-byte marker, scoring still a `TODO`. Pass one anyway (it costs a line and 0.6 keeps the parameter), but never rely on it to break a tie: a container whose marker isn't registered is one the probe will mis-assign, silently and to whichever reader matches first. **This tree carried two Symphonia majors for that reason and no longer does** — rodio is gone entirely and everything decodes through `player::decode` against 0.6, whose probe scores each candidate against the frames that follow it. Read that module before touching either decoder.
- Use `MediaSourceStream` (not `BufReader`) — it provides optimized buffering for multimedia I/O
- Search for the first audio track explicitly — default track may be video in container formats

### Decode Loop Pattern

```
loop {
    match format_reader.next_packet() {
        Ok(packet) => match decoder.decode(&packet) {
            Ok(decoded) => { /* process audio buffer */ }
            Err(DecodeError) => continue,  // skip corrupted frames
            Err(e) => return Err(e),
        },
        Err(IoError) if end_of_stream => break,
        Err(ResetRequired) => { /* reset decoder, re-read format */ }
        Err(e) => return Err(e),
    }
}
```

### Seeking

- **Always reset the decoder after a seek**, and don't be talked out of it. `FormatReader::seek`'s
  own docs say every decoder consuming that reader should be reset, and in 0.6.1 the codecs holding
  no state across packets — FLAC, ALAC, PCM, ADPCM — document their `reset` as doing nothing, so
  the blanket call costs them a vtable hop. The ones it is *for* are MP3, which rebuilds its whole
  state, and AAC and Vorbis, which clear an overlap-add buffer that otherwise blends the audio
  either side of the jump. Reset selectively and those two are exactly what you lose.
  [symphonia#274](https://github.com/pdeljanov/Symphonia/issues/274) is not an argument against it:
  it reports MP3 seeks popping, and says the fault is *not* seen with MKA/Vorbis or M4A/AAC. It was
  read here once as saying a reset sends some containers back to the start, which it does not say
  and no decoder's `reset` could do
- Use `FormatReader::seek()` with `SeekTo::Time` for timestamp-based seeking, and `SeekMode::Accurate`
- **Clamp a seek short of a stated length, not onto it.** The last frame is not somewhere a demuxer
  can land: the seek answers out of range and the failed attempt still parks the reader at the end,
  so the next pull reads as the track finishing. A slider dragged to its right edge asks for exactly
  the length, so this is the common case rather than an edge one — `file_decode::SEEK_END_MARGIN`
- **A demuxer seek lands on a packet boundary, so trim the head yourself.** Without it every seek
  replays the tail of what came before. `required_ts - actual_ts` through the track's timebase gives
  the frames to drop. Note that neither reference player does this: `symphonia-play` skips whole
  packets and says in its own comment that it should not, and termusic seeks `Coarse` and skips whole
  packets too. rodio's `refine_position` was frame-accurate and was the bar; `file_decode::try_seek`
  is what holds it now, and `file_decode_tests::a_seek_lands_on_the_frame_it_asked_for` is what says
  so, since nothing else in the tree can tell you the trim went missing

### Gapless Support

- **0.6 moved the flag rather than dropping it**: it is `AudioDecoderOptions::gapless`, not
  `FormatOptions::enable_gapless`, and it **defaults to `true`**, so `AudioDecoderOptions::default()`
  needs nothing enabled
- **But only two decoders act on it**, MP3 and Vorbis. AAC, ALAC, FLAC, PCM and ADPCM ignore
  `opts.gapless` outright, and `symphonia-format-isomp4` populates neither `Packet::trim_start`/
  `trim_end` nor `Track::delay`/`padding` (it parses the edit list into `TrakAtom.edts` and never
  reads it; iTunSMPB is not parsed at all). This is not a 0.6 regression: 0.5 gated the
  same two demuxers on `FormatOptions::enable_gapless`, and MP3 got *better*, since 0.6 emits the
  trims unconditionally under a default-on flag
- **`Track::delay`/`padding` is advisory metadata rather than something applied.** CAF fills both
  from its packet table and no decoder it feeds ever uses them; Opus-in-Ogg fills `delay` from
  `pre_skip` for a decoder 0.6.1 does not ship. So AAC is trimmed here rather than upstream, in
  `player::aac_trim`, which reads the two places a file states its padding and hands `file_decode` a
  head and a playable length; that module argues the whole design and the numbers, and
  `.claude/rules/audio-stack.md` says why it sits outside the shared `decode`. rox reached the same
  conclusion from the other direction, distrusting the trimming enough to plan its own before
  verifying that MP3's holds, and then shipping only the harness that checks it

### Performance

- Reuse allocated buffers across decode iterations where possible
- For metadata-only reads, skip decoding entirely — use format reader to read metadata packets

### Sample Format Conversion

- Symphonia decoded buffers are `AudioBuffer<T>` — call `.convert::<f32>()` or use `SampleBuffer<f32>` for interleaved output
- `SampleBuffer::copy_interleaved_ref(&decoded)` writes decoded frames into a flat `&[f32]` slice, which is what a deck is appended

### Error Recovery

- `DecodeError` on a packet — log and `continue`; do not abort the decode loop. A mount joined
  mid-frame opens with a few, and a file with a damaged frame plays past it the way every other
  player does
