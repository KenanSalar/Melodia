//! The claim the signal path panel makes when it reads Bit-perfect: with nothing the user chose
//! in the way, the bytes an exclusive backend writes are the file's own samples.
//!
//! Driven through the real engine on a device-free mixer, from decoder through `EqSource`, the
//! deck and the sum, then through the dither and `encode` as both exclusive writers do. The
//! fixtures are written here rather than committed: noise, so every bit of a sample is exercised,
//! with the format's integer extremes placed at the start.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use melodia_engine::player::engine::backend::PlaybackEngine;
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::dither::Dither;
use melodia_playback::player::playback::output::encode::{self, DeviceFormat};
use melodia_playback::player::playback::output::mixer::{self, MixerPull};
use melodia_playback::player::playback::replaygain::TrackReplayGain;

mod common;

const CHANNELS: u16 = 2;
const FRAMES: usize = 8_192;

/// How long an append may take to land while the mixer is turned by hand.
const START_BUDGET: Duration = Duration::from_secs(5);

/// A writer's block, the default period at 48 kHz. Its size only decides which samples share a
/// block, and a bit-perfect stream has none for the dither to move.
const WRITER_BLOCK_FRAMES: usize = 960;

/// One fixture: a WAV of `bits`-wide noise, and the device format that should carry it back out.
struct Case {
    rate: u32,
    bits: u8,
    format: DeviceFormat,
}

const S16: Case = Case { rate: 44_100, bits: 16, format: DeviceFormat::S16 };
const S24: Case = Case { rate: 96_000, bits: 24, format: DeviceFormat::S24Packed };

/// `FRAMES` frames of noise from a fixed seed, the two extremes of a `bits`-wide integer first.
fn noise(bits: u8) -> Vec<i32> {
    let max = (1_i32 << (bits - 1)) - 1;
    let span = 1_i64 << bits;
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut samples = vec![max, -max - 1];
    while samples.len() < FRAMES * usize::from(CHANNELS) {
        state =
            state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let drawn = i64::try_from(state >> 33).unwrap_or(0) % span - span / 2;
        samples.push(i32::try_from(drawn).unwrap_or(0));
    }
    samples
}

/// `samples` as the little-endian bytes of a `bits`-wide PCM stream.
fn pcm(samples: &[i32], bits: u8) -> Vec<u8> {
    let width = usize::from(bits / 8);
    samples.iter().flat_map(|s| s.to_le_bytes()[..width].to_vec()).collect()
}

fn write_wav(path: &Path, rate: u32, bits: u8, data: &[u8]) -> std::io::Result<()> {
    let block_align = CHANNELS * u16::from(bits / 8);
    let data_len = u32::try_from(data.len()).map_err(std::io::Error::other)?;
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&u16::from(bits).to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(data);
    std::fs::write(path, wav)
}

/// Starts `path` on a fresh engine and returns everything the mixer produced, as `format` bytes.
///
/// The append only lands once the mixer is pulled, and the pulling is ours, so the start runs on
/// another thread and every block pulled while waiting is kept: the track begins inside them.
fn play_through(path: &Path, case: &Case, eq_on: bool) -> std::io::Result<Vec<u8>> {
    let (mixer, mut pull) = mixer::pair(DECK_COUNT, common::shape(CHANNELS, case.rate));
    let engine = PlaybackEngine::new(&mixer, tokio::runtime::Handle::current())
        .map(Arc::new)
        .map_err(std::io::Error::other)?;
    if eq_on {
        engine.set_eq_enabled(true);
        engine.set_eq_band(3, 6.0);
    }

    let starting = Arc::clone(&engine);
    let path = path.to_string_lossy().into_owned();
    let start = std::thread::spawn(move || {
        starting.play_media(&path, 1.0, 1.0, None, TrackReplayGain::default())
    });
    let mut out = Vec::new();
    let began = Instant::now();
    while !start.is_finished() {
        assert!(began.elapsed() < START_BUDGET, "the track never started");
        out.extend(common::pull(&mut pull, 256));
    }
    let started = start.join().map_err(|_| std::io::Error::other("play_media panicked"))?;
    started.map_err(std::io::Error::other)?;
    out.extend(drain(&mut pull));

    let mut dither = Dither::default();
    for block in out.chunks_mut(WRITER_BLOCK_FRAMES * usize::from(CHANNELS)) {
        dither.quantize(block, case.format);
    }
    let mut bytes = Vec::new();
    encode::encode(&out, case.format, &mut bytes);
    Ok(bytes)
}

/// The rest of the track and a block of the silence after it.
fn drain(pull: &mut MixerPull) -> Vec<f32> {
    common::pull(pull, (FRAMES + 4_096) * usize::from(CHANNELS))
}

/// Where `expected` starts inside `output`, if it appears there whole.
fn find(output: &[u8], expected: &[u8]) -> Option<usize> {
    output.windows(expected.len()).position(|window| window == expected)
}

fn round_trip(case: &Case, eq_on: bool) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(format!("noise-{}.wav", case.bits));
    let expected = pcm(&noise(case.bits), case.bits);
    write_wav(&path, case.rate, case.bits, &expected)?;
    Ok((play_through(&path, case, eq_on)?, expected))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_16_bit_file_reaches_the_device_bit_for_bit() -> std::io::Result<()> {
    let (output, expected) = tokio::task::block_in_place(|| round_trip(&S16, false))?;

    assert!(find(&output, &expected).is_some(), "the file's PCM is not in the output whole");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_24_bit_file_reaches_the_device_bit_for_bit() -> std::io::Result<()> {
    let (output, expected) = tokio::task::block_in_place(|| round_trip(&S24, false))?;

    assert!(find(&output, &expected).is_some(), "the file's PCM is not in the output whole");
    Ok(())
}

/// The check has to be able to fail: an EQ band that is doing something changes the samples.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_active_equalizer_is_not_bit_perfect() -> std::io::Result<()> {
    let (output, expected) = tokio::task::block_in_place(|| round_trip(&S16, true))?;

    assert!(find(&output, &expected).is_none(), "an active EQ left the samples untouched");
    Ok(())
}
