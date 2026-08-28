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

- **A `Hint` does not steer 0.5's probe** — `Probe::format` takes it as `_hint` and resolves the format by matching a two-byte marker, scoring still a `TODO`. Pass one anyway (it costs a line and 0.6 keeps the parameter), but never rely on it to break a tie: a container whose marker isn't registered is one the probe will mis-assign, silently and to whichever reader matches first. **This tree carries two Symphonia majors for that reason** — rodio's 0.5 for local files, where the extension is known and the formats are well-marked, and 0.6 in `player::stream_decode` for live streams, where neither holds. Read that module before touching either.
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

- **Always reset the decoder after seeking** — stale internal state causes artifacts
- Use `FormatReader::seek()` with `SeekTo::Time` for timestamp-based seeking

### Gapless Support

- Enable `FormatOptions::enable_gapless = true` for seamless track transitions
- Symphonia handles encoder delay/padding trimming automatically when gapless is enabled

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
