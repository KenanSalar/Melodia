//! A tone at a 96 kHz source's Nyquist, through one voice onto a 48 kHz device, no audio device.
//!
//! **Nothing else in this binary may prepare the kernel's table.** It is a process-wide `LazyLock`,
//! and the unit suites prepare it for cases of their own, so only a process where a pick is the one
//! thing that could have built it can tell a pick that does from one that leaves the audio thread
//! falling back to linear.

use melodia_audio::player::source::audio::SourceFormat;
use melodia_audio::player::source::prebuffer::{PrebufferSource, StreamShared};
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::convert::Resampler;
use melodia_playback::player::playback::output::mixer;

mod common;
use common::{pull, shape};

const SOURCE_RATE: u32 = 96_000;
const DEVICE_RATE: u32 = 48_000;

/// The tone's level either side of zero.
const LEVEL: f32 = 0.5;

/// Source frames fed up front, well inside the ring and past everything the pulls below take.
const SOURCE_FRAMES: usize = 24_000;

/// Output frames pulled before the level is read, past the kernel's reach into the tone.
const SETTLE: usize = 1_024;

/// Output frames the level is read over.
const MEASURED: usize = 8_000;

/// The tone, already fully buffered and closed, so the voice plays it without a feed thread.
fn nyquist_tone() -> PrebufferSource {
    let shared = StreamShared::new();
    let (source, writer) =
        PrebufferSource::new(shared.clone(), shape(1, SOURCE_RATE), SourceFormat::F32);
    for sample in [LEVEL, -LEVEL].repeat(SOURCE_FRAMES / 2) {
        assert!(writer.push(sample), "the ring must take the whole tone up front");
    }
    shared.finish();
    source
}

/// What the device is handed of the tone through a voice set to `resampler`, once it has settled.
fn played_through(resampler: Resampler) -> std::io::Result<Vec<f32>> {
    let (mixer, mut out) = mixer::pair(DECK_COUNT, shape(1, DEVICE_RATE));
    let voice =
        mixer.voice(0).ok_or_else(|| std::io::Error::other("a two-voice mixer has a voice 0"))?;
    voice.set_resampler(resampler);
    voice.append(nyquist_tone());
    voice.play();

    let _ = pull(&mut out, SETTLE);
    Ok(pull(&mut out, MEASURED))
}

/// A band-limited pick builds the table before the audio thread asks for it. Left unbuilt, the
/// thread interpolates linearly instead and the tone plays at full level.
#[test]
fn a_band_limited_voice_stops_a_tone_at_the_sources_nyquist() -> std::io::Result<()> {
    let heard = played_through(Resampler::BandLimited)?;

    let loudest = heard.iter().fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()));
    assert!(loudest < 1e-3, "the tone came through at {loudest}");
    Ok(())
}

/// Linear at a step of two lands on every other sample, so the tone folds into a constant at its
/// full level: the aliasing the kernel is there to stop, and the cost of the cheaper pick.
#[test]
fn a_linear_voice_decimates_it_to_full_level() -> std::io::Result<()> {
    let heard = played_through(Resampler::Linear)?;

    let off_level = heard.iter().find(|sample| sample.to_bits() != LEVEL.to_bits());
    assert_eq!(off_level, None, "every frame lands on a sample at {LEVEL}");
    Ok(())
}
