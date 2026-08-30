---
paths:
  - src/player/**/*.rs
  - tests/**/*.rs
---

# Rodio + Symphonia Best Practices

## Rodio — Audio Playback

### Lifecycle

- `MixerDeviceSink` must outlive all `Player` instances — store as `_speakers` field in a long-lived struct
- Call `speakers.log_on_drop(false)` to suppress stderr noise on drop
- Stop playback with `Player::clear()` — it removes all sources and pauses automatically. Never drop and recreate the Player
- All samples are `f32` exclusively since Rodio 0.22

### API Names (0.22+)

- `Sink` was renamed to `Player`
- `OutputStream` was renamed to `MixerDeviceSink`

### Gapless Playback

- Keep the queue 2 tracks deep via `Player::append()` — append the next track before the current one ends
- Detect track end via position-tick timer (e.g., `tokio::time::interval(1s)`), not callbacks or bus events
- Pre-decode or buffer the next track to minimize transition gaps

### Thread Safety

- `Player` is both `Send` and `Sync` — can be shared across threads without a `Mutex`
- Audio output runs on its own thread — avoid blocking it with expensive operations

## Symphonia — Audio Decoding

### Format Probing

- **A `Hint` does not steer 0.5's probe** — `Probe::format` takes it as `_hint` and resolves the format by matching a two-byte marker, scoring still a `TODO`. Pass one anyway (it costs a line and 0.6 keeps the parameter), but never rely on it to break a tie: a container whose marker isn't registered is one the probe will mis-assign, silently and to whichever reader matches first. **This tree carried two Symphonia majors for that reason and no longer does** — rodio is cut to its `playback` feature and everything decodes through `player::decode` against 0.6, whose probe scores each candidate against the frames that follow it. Read that module before touching either decoder.
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

- **Reset the decoder after a seek only where the codec needs it** — the blanket advice is wrong.
  Resetting misbehaves for some containers, sending them back to the start
  ([symphonia#274](https://github.com/pdeljanov/Symphonia/issues/274)); `player::file_decode` resets
  for MP3 and nothing else, which is what a working implementation does
- Use `FormatReader::seek()` with `SeekTo::Time` for timestamp-based seeking, and `SeekMode::Accurate`
- **A demuxer seek lands on a packet boundary, so trim the head yourself.** Without it every seek
  replays the tail of what came before. `required_ts - actual_ts` through the track's timebase gives
  the frames to drop. Note that neither reference player does this: `symphonia-play` skips whole
  packets and says in its own comment that it should not, and termusic seeks `Coarse` and skips whole
  packets too. rodio's `refine_position` is frame-accurate, and that is the bar to keep

### Gapless Support

- **0.6 moved the flag rather than dropping it**: it is `AudioDecoderOptions::gapless`, not
  `FormatOptions::enable_gapless`, and it **defaults to `true`** — so `AudioDecoderOptions::default()`
  already trims encoder delay and padding and there is nothing to enable
- `Track::delay` and `Track::padding` carry the same numbers, so the trim can be checked rather than
  reimplemented. Worth checking: rox distrusts Symphonia's trimming enough to do its own, citing an
  MP3 LAME-header gap

### Performance

- Reuse allocated buffers across decode iterations where possible
- For metadata-only reads, skip decoding entirely — use format reader to read metadata packets

### Sample Format Conversion

- Symphonia decoded buffers are `AudioBuffer<T>` — call `.convert::<f32>()` or use `SampleBuffer<f32>` for interleaved output
- `SampleBuffer::copy_interleaved_ref(&decoded)` writes decoded frames into a flat `&[f32]` slice ready for Rodio's `append()`

## Rodio — Additional Patterns

### Volume & Effects

- `Player::set_volume(f32)` — linear amplitude scale (1.0 = unity gain, >1.0 amplifies)
- Chain sources with `.amplify()`, `.fade_in()`, `.delay()` before appending to `Player`
- `Player::speed(ratio)` — playback speed without pitch shift is not built-in; use an external resampler

### Seeking

- `Player::try_seek(Duration)` — seeks within the current source; returns `Result<(), SeekError>`
- Requires `DecoderBuilder::with_seekable(true)` and `.with_byte_len(len)` for proper operation
- Saturates at source end if duration is known (seeking past end seeks to end)

### Error Recovery

- `DecoderError::InvalidData` on a packet — log and `continue`; do not abort the decode loop
- `rodio::PlayError` on `append()` is rare but possible if the output device disconnects — handle gracefully
