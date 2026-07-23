# Opus Support

Working doc. Delete when the feature ships.

Adds `.opus` playback via rodio's `symphonia-libopus` feature (libopus through
`symphonia-adapter-libopus`), gated on the release of **rodio 0.23**.

All upstream facts below were verified **2026-07-23**. Anything marked
⚠️ **re-verify** is expected to drift before 0.23 lands — check it again on the day
rather than trusting this doc.

---

## Why this shape

Three findings decide the approach:

1. **The container half already works.** `symphonia-format-ogg` ships a complete Opus
   mapper (`src/mappings/opus.rs`) and already lists `"opus"` in its extension set
   (`demuxer.rs:351`). It parses `OpusHead`, sets `CODEC_TYPE_OPUS`, 48 kHz, channel
   count, and `with_delay(pre_skip)` — so demuxing *and* gapless pre-skip trimming are
   correct today, on the symphonia 0.5.5 we already compile. The only missing piece is
   a `Decoder` registered for `CODEC_TYPE_OPUS`.

2. **rodio has already done the wiring.** PR
   [#851](https://github.com/RustAudio/rodio/pull/851) (merged 2026-03-11) makes
   `Settings::default()` register the adapter automatically when the feature is on:

   ```rust
   codec_registry: {
       let mut codec_registry = CodecRegistry::new();
       register_enabled_codecs(&mut codec_registry);
       #[cfg(feature = "symphonia-libopus")]
       codec_registry.register_all::<symphonia_adapter_libopus::OpusDecoder>();
       Registry::new(codec_registry)
   }
   ```

   It is unreleased: last release is 0.22.2 (2026-03-05), six days before the merge.

3. **Our decode path needs no change.** `decode_file` already passes the file
   extension as the hint, and rodio's docs list `"opus"` as the hint string gated on
   `symphonia-libopus`. So Phase 2 is genuinely a feature flag plus one entry in
   `AUDIO_EXTENSIONS`.

The work that *isn't* free is everything downstream: the C build in CI, and two
Opus-specific loudness quirks that silently degrade features we already ship
(Phase 4).

**Native Symphonia Opus is not the plan.** `symphonia-codec-opus/src/lib.rs` is a
1-byte placeholder; every commit touching that crate is housekeeping. Both paths
terminate at the same Symphonia `Decoder` trait, so swapping to a first-party decoder
later is a dependency line and a registration call — see Phase 8.

---

## Phase 0 — Trigger

Not started. Nothing to do until rodio 0.23 is on crates.io.

- [ ] `cargo search rodio` reports `0.23.x` (watch
      [releases](https://github.com/RustAudio/rodio/releases))
- [ ] Read the released `CHANGELOG.md` + `UPGRADE.md` in full — the notes captured in
      Phase 1 are a snapshot of `main`, not the final release
- [ ] ⚠️ **re-verify** which Symphonia the release pins. `main` currently pins
      symphonia 0.5.5 + `symphonia-adapter-libopus` 0.2 and registers via
      `register_all::<OpusDecoder>()`. If rodio moves to symphonia 0.6 first, the
      adapter goes to 0.3 and the call becomes `register_audio_decoder::<OpusDecoder>()`.
      Either way it's internal to rodio — but it changes our transitive graph.

---

## Phase 1 — Upgrade to rodio 0.23 (no Opus yet)

Keep this phase Opus-free so a regression here is unambiguous.

**Our rodio surface is small** — every symbol we touch, verified present in `main` with
unchanged signatures:

| Site | Symbols |
|---|---|
| `src/state/mod.rs:130` | `DeviceSinkBuilder::from_default_device`, `with_error_callback`, `open_stream`, `MixerDeviceSink`, `log_on_drop` |
| `src/player/rodio_backend.rs` | `Decoder`, `Decoder::builder`, `Player`, `mixer::Mixer` |
| `src/player/decks.rs` | `Player::{connect_new, append, clear, play, pause, set_volume, set_speed, try_seek, get_pos, len, empty, is_paused}`, `Source` |
| `src/player/equalizer.rs` | `Source`, `SeekError`, `ChannelCount`, `Sample`, `SampleRate` |

**Breaking changes in `UPGRADE.md` as of 2026-07-23 — none of the three touch us:**

- `stream::supported_output_configs` removed — we don't call it.
- `Done` now takes a callback instead of an `Arc<AtomicUsize>` — we don't use `Done`.
- `Zero::new_samples()` returns `Result` — we don't use `Zero`.

⚠️ **re-verify:** that list will have grown by release. `main` is mid-flight.

**Behavioural changes that do land on us:**

- [ ] **`Source` trait shape.** Required methods in `main` are still
      `current_span_len` / `channels` / `sample_rate` / `total_duration`, with
      `try_seek`, `size_hint`, `is_exhausted` defaulted. `EqSource` implements exactly
      that set — expected clean, confirm.
- [ ] **Mixer still sums without clamping.** `Mixer::sum_current_sources` in `main` is
      still a bare `sum += value` with no clamp. The complementary-linear crossfade
      curve depends on this (post-clamp decks summing ≤ 1.0 is what prevents clipping).
      If 0.23 adds a clamp, the crossfade design needs revisiting before anything else.
      **Check this first — it's the one change that would invalidate an architectural
      assumption.**
- [ ] **Complete-frame guarantees.** 0.23 documents that sources must return complete
      frames and fixes decoders + `TakeDuration` to comply. This *strengthens* the
      `frame_phase == 0` generation-poll invariant in `EqSource::next`; confirm it
      doesn't change the arithmetic.
- [ ] **Span-boundary handling.** 0.23 fixes sources to handle sample-rate and
      channel-count changes at span boundaries. `EqSource` builds biquad coefficients
      from `Source::sample_rate()` — check whether a mid-source spec change can now
      reach us, and whether `rebuild` needs to key on rate as well as generation.
- [ ] **Default rate 44.1 → 48 kHz** and `open_sink_or_fallback` now tries 48 kHz then
      44.1 kHz before the device max. We call `from_default_device().open_stream()`, so
      confirm which config we end up on and that the limiter's per-channel-rate
      constants still line up.
- [ ] **`SampleRateConverter` now wraps a `Source` and takes a `ResampleConfig`**
      (rubato-backed). We don't call it directly, but it's now in the playback path
      whenever source rate ≠ device rate. Relevant to Phase 2: **Opus always decodes at
      48 kHz**, so on a 44.1 kHz device every Opus track goes through the resampler —
      a path our current formats mostly avoid.

**Steps**

- [ ] Bump `rodio` in `Cargo.toml` to the exact released `0.23.x` (full `x.y.z`, per
      dep convention). Keep the existing per-codec feature list unchanged.
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test` — pay attention to `player/tests/{crossfade,equalizer,handlers,actions}_tests.rs`
- [ ] `tests/crossfade.rs` drives the mixer directly — the highest-signal test for the
      no-clamp property and for frame parity.

**Gate:** manual playback pass on the existing eight formats — gapless transition,
crossfade transition, fade-on-pause, seek, speed change at 0.25× and 2×, EQ sweep,
ReplayGain on an album-tagged FLAC. Then a release build + `/usr/bin/time -v` RSS
reading for the record.

---

## Phase 2 — Turn on Opus decode

- [ ] Add `"symphonia-libopus"` to the `rodio` feature list in `Cargo.toml`, with a
      comment covering: what it pulls (`symphonia-adapter-libopus` → `opusic-sys` →
      vendored libopus, built by CMake), and why the adapter rather than native
      Symphonia.
- [ ] Add `"opus"` to `AUDIO_EXTENSIONS` (`src/media/mod.rs:10`). That single const is
      the whole gate — library walk, watcher, DnD import and Browse all route through
      `is_audio_extension`.
- [ ] Consider `"oga"` and `"spx"` in the same edit. Symphonia's Ogg demuxer already
      claims both; `.oga` is commonly Opus or Vorbis and would work, `.spx` (Speex) has
      no decoder and would fail at playback — **so add `oga`, not `spx`.**
- [ ] Verify `decode_file` needs no change: it passes the extension as `with_hint`, and
      `"opus"` is the documented hint. Expected zero-diff.

**Do not** enable any of `opusic-sys`'s `no-hardening`, `no-stack-protector`,
`no-fortify-source`, or `no-simd` features. Defaults keep the hardening on; we're
compiling C into a binary that parses files from the user's disk.

**Gate:** a `.opus` file scans, appears with correct duration/metadata/artwork, plays,
seeks, and survives a gapless transition into and out of an MP3 neighbour.

---

## Phase 3 — Scan and playback surface

- [ ] **Metadata.** Lofty 0.24 already ships `src/ogg/opus/` — title/artist/album/
      artwork/comments come through the existing `extract_metadata` path with no change.
      Verify against a real file rather than assuming.
- [ ] **Duration.** Ogg Opus duration comes from the final page granule position;
      confirm `TrackSummary.duration_ms` is sane for VBR Opus. Our `with_byte_len` hint
      exists for MP3/Vorbis VBR estimation and shouldn't interfere.
- [ ] **EQ.** Opus is always 48 kHz, so all ten bands sit below Nyquist and the
      band-skip path never triggers. No change expected — worth one listen.
- [ ] **Crossfade / gapless.** `with_delay(pre_skip)` in the mapper means Symphonia
      trims encoder delay for us under `with_gapless(true)`. Verify an Opus→Opus gapless
      transition has no audible seam, and that an Opus track crossfades correctly (the
      ramp counts media samples, which is rate-independent, so this should be clean).
- [ ] **BLAKE3 / moved-file detection, watcher, playlists (M3U8), smart playlists** —
      all format-agnostic, no work expected.

---

## Phase 4 — Opus loudness (the part that silently breaks a shipped feature)

Opus does **not** use `REPLAYGAIN_*` tags. Two separate mechanisms, and we currently
honour neither.

### 4a — `R128_TRACK_GAIN` / `R128_ALBUM_GAIN` (required)

`src/media/metadata.rs:210-213` reads only `ItemKey::ReplayGain{Track,Album}{Gain,Peak}`.
Lofty 0.24 has **no `R128` handling at all** (grepped: zero hits) — it maps
`REPLAYGAIN_TRACK_GAIN` in VorbisComments to `ItemKey::ReplayGainTrackGain`, and leaves
`R128_TRACK_GAIN` as an unmapped custom comment.

Consequence: an Opus file tagged by `opusenc`, `loudgain -o`, or `rsgain` carries
`R128_TRACK_GAIN` and reads as *untagged* — `rg_gain` silently falls back to unity while
every other format in the library is normalised. Volume jumps on every Opus track.

- [ ] Read `R128_TRACK_GAIN` / `R128_ALBUM_GAIN` from the Vorbis comments directly
      (they're plain string items; reach past `ItemKey` to the raw key).
- [ ] Convert: the value is **Q7.8 fixed point** — dB × 256, as a signed integer string.
      `gain_db = value as f64 / 256.0`.
- [ ] **Add +5 dB.** R128 references −23 LUFS; ReplayGain 2.0 references −18 LUFS. Without
      the offset every Opus track plays ~5 dB below the rest of the library — which looks
      like the feature working but wrong, the worst failure mode.
- [ ] Peak: R128 defines no peak field. Leave `replaygain_*_peak` as `None`; the
      prevent-clipping path already handles an unknown peak.
- [ ] Prefer `REPLAYGAIN_*` when both are present (some taggers write both) — that path
      is already reference-aligned and needs no offset.
- [ ] Unit-test the conversion in `src/media/tests/metadata_tests.rs`: a known Q7.8
      value round-trips to the expected dB, and the −23→−18 offset is applied exactly
      once.

### 4b — `OpusHead` output gain (optional, lower priority)

RFC 7845 puts a mandatory-to-apply gain in the identification header. **Neither of our
libraries applies or exposes it:**

- `symphonia-format-ogg` `mappings/opus.rs:73` — `let _ = reader.read_u16()?;`
- Lofty `src/ogg/opus/properties.rs:105` — `let _output_gain = …`, no getter on
  `OpusProperties`

So a file with a non-zero header gain plays at the wrong level. In practice most taggers
write the comment tag instead of the header, so this is rarer than 4a.

- [ ] Decide whether to handle it. If yes: read bytes 16–17 of the identification packet
      as little-endian `i16`, Q7.8 dB, and fold it into the baked `TrackReplayGain` at
      scan time — we already have the per-source gain mechanism, so it's a scan-side read
      plus an addition, not a DSP change.
- [ ] If deferred, note it in `CLAUDE.md` as a known gap rather than leaving it silent.

---

## Phase 5 — Tag editing

The Edit Track Information dialog writes through `src/media/tag_writer.rs`, which
targets the **primary tag type**. For Ogg Opus that's VorbisComments.

- [ ] Verify `apply_to_file` resolves the primary tag correctly for Opus and that a
      round-trip edit (title, artist, album artist, composer, BPM, lyrics) survives.
- [ ] BPM: the ID3v2-specific `IntegerBpm` + `Bpm` double-write shouldn't be needed here
      (VorbisComments has a `Bpm` key) — confirm no `UnsupportedFields` entries come back.
- [ ] Lyrics key on Vorbis is `Lyrics`, not `UnsyncLyrics` — the existing tag-type branch
      already covers this; confirm it fires.
- [ ] MusicBrainz Recording ID: the `UFID` unchecked-insert special case in
      `apply_recording_id` is ID3v2-only. Opus goes through the normal text path —
      confirm MBID auto-tagging writes and re-reads correctly, since LB love-sync depends
      on it.
- [ ] Cover art: Opus stores pictures as base64 `METADATA_BLOCK_PICTURE` comments.
      Confirm `read_cover_art` + replace/remove behave, and that the M4A-style
      `pic_type` flattening quirk does **not** apply here.
- [ ] Remember the export caveat: a tag edit changes `file_hash`, staling
      `#MELODIA-HASH` lines in previously exported `.m3u8`.

---

## Phase 6 — CI and packaging

`opusic-sys` 0.7.3 defaults to `bundled`, which builds vendored libopus **via CMake**
(`[build-dependencies.cmake]`) as a static library.

- [ ] **CMake availability.** Preinstalled on the runner images we use:
      `ubuntu-latest` 3.31.6, `windows-2025` 3.31.6, `windows-11-arm` 4.4.0. libopus
      declares `cmake_minimum_required(VERSION 3.16)`, so CMake 4.x is fine (4.x only
      rejects `<3.5`). ⚠️ **re-verify** `ubuntu-24.04-arm` specifically — it's the one
      image I didn't confirm, and it carries four of the ten release slots.
- [ ] If any slot lacks it, add cmake to `.github/actions/linux-system-deps/action.yml`
      (alongside `libasound2-dev` et al.) rather than to individual jobs.
- [ ] **Build time.** The vendored C build runs once per cold cache, across all ten
      release slots and both PR-validation jobs. Check the hit on `Swatinem/rust-cache`
      warm runs — if it rebuilds every time, that's a cache-key problem worth fixing.
- [ ] **Artifact size.** libopus static is small, but measure the delta on the tarball,
      AppImage, RPM, DEB and MSI. The updater downloads whole artifacts.
- [ ] **LTO interaction.** `opusic-sys` explicitly disables interprocedural optimisation
      for the opus build and strips `-flto` from `CFLAGS`. Our `lto = "fat"` applies to
      Rust codegen units and doesn't conflict — but if CI sets `CFLAGS`, expect a
      `cargo:warning` and confirm it's benign.
- [ ] **Licensing.** libopus and `opusic-sys` are BSD-3-Clause; the adapter is
      MIT OR Apache-2.0. All compatible with AGPL-3.0-or-later; the package `License:`
      fields stay as they are. Consider adding the BSD notice to the RPM/DEB doc dir
      for attribution hygiene.
- [ ] **`cargo audit`** runs in `release.yml` — confirm the new C-bearing crates come
      back clean.
- [ ] Update `packaging/com.github.kenansalar.melodia.metainfo.xml` and the
      `[package.metadata.deb]` `extended-description` in `Cargo.toml`, which both list
      supported formats explicitly.

---

## Phase 7 — Tests

- [ ] Add a small Opus fixture. Generate rather than commit a found file:
      `gst-launch-1.0 audiotestsrc num-buffers=200 ! audioconvert ! opusenc ! oggmux ! filesink location=…`
      or `ffmpeg`/`opusenc` if preferred.
- [ ] `src/media/tests/scanner_tests.rs` asserts `files.len() == AUDIO_EXTENSIONS.len()`
      — it picks up new extensions automatically, but confirm it still passes.
- [ ] R128 conversion unit test (Phase 4a).
- [ ] Decode smoke test: build a `Decoder` over the fixture and pull frames — catches a
      missing feature flag or an unregistered codec without needing an audio device.
- [ ] `tests/headless.rs` runs under CI's ALSA null device; make sure nothing new needs a
      real device.

---

## Phase 8 — Docs and exit

- [ ] `README.md` — format list in the feature section.
- [ ] `CLAUDE.md`:
      - Symphonia formats bullet: add Opus, and note the decoder is libopus via the
        adapter, not Symphonia native.
      - ReplayGain section: document the R128 read path and the +5 dB reference offset.
        That offset is exactly the kind of thing that looks like a bug to the next
        reader.
      - Any deferred item from Phase 4b, as a known gap.
      - "pure-Rust backend" in the header: reword. It's already imprecise — the binary
        statically links SQLite (`libsqlite3.a`) and aws-lc-sys — but adding a *media*
        C dependency makes the claim actively misleading. "No WebView, no IPC, no
        FFmpeg/GStreamer media stack" is what's actually true and worth defending.
- [ ] Delete this file.

**Future swap to native Symphonia Opus.** When `symphonia-codec-opus/src/lib.rs` stops
being 1 byte and ships: drop the `symphonia-libopus` feature, enable the native codec
feature, drop the CMake CI step. Both register into the same `CodecRegistry` behind the
same `Decoder` trait, so nothing in `rodio_backend.rs`, `decks.rs` or `equalizer.rs`
changes. Phase 4's R128 work is decoder-independent and stays either way. Worth a note
in `CLAUDE.md` next to the winit-fork retirement condition — same shape of "delete this
when upstream lands".

---

## Appendix — verified references

| Claim | Source |
|---|---|
| rodio Opus merged, unreleased | [PR #851](https://github.com/RustAudio/rodio/pull/851), merged 2026-03-11; last release 0.22.2, 2026-03-05 |
| Feature auto-registers the decoder | rodio `main` `src/decoder/builder.rs`, `Settings::default()` |
| Manual registration escape hatch | `DecoderBuilder::with_symphonia_decoder::<D>()`, same file |
| `"opus"` is the hint string | rodio master `_autodocs/configuration.md` (via Context7) |
| Ogg Opus demuxing already works | `symphonia-format-ogg-0.5.5/src/mappings/opus.rs`, `demuxer.rs:351` |
| Pre-skip handled | same file, `.with_delay(u32::from(pre_skip)).with_sample_rate(48_000)` |
| Mixer sums without clamping | rodio `main` `src/mixer.rs::sum_current_sources` |
| libopus built by CMake | `opusic-sys` 0.7.3 `Cargo.toml`, `bundled = ["dep:cmake"]`; `build.rs` |
| libopus needs CMake ≥3.16 | `xiph/opus` `CMakeLists.txt` |
| Lofty has no R128 support | `lofty-0.24.0/src/` — zero `R128` matches |
| Header output gain discarded | `symphonia .../mappings/opus.rs:73`; `lofty .../ogg/opus/properties.rs:105` |
| Symphonia native Opus is a stub | `symphonia-codec-opus/src/lib.rs` is 1 byte; [issue #8](https://github.com/pdeljanov/Symphonia/issues/8) open since 2020-04-11 |
