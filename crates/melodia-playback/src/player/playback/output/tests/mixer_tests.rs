//! Tests for the sum.
//!
//! `crates/melodia/tests/crossfade.rs` covers the same ground through the whole chain, with a real
//! decoder and a real ramp; what is here is the property on its own, so a change to the sum fails
//! at the sum.

use std::sync::Arc;

use super::{LOCKSTEP_FRAMES, Mixer, pair};
use crate::player::playback::output::voice::Voice;
use crate::player::playback::tests::helpers::{TestSource, bits, shape};
use melodia_audio::player::source::audio::Shape;
use melodia_core::error::AppError;

const RATE: u32 = 48_000;
const VOICES: usize = 2;

/// `channels` at [`RATE`], which every device in this suite runs at.
fn device(channels: u16) -> Shape {
    shape(channels, RATE)
}

fn voice_at(mixer: &Mixer, index: usize) -> Result<Arc<Voice>, AppError> {
    mixer
        .voice(index)
        .ok_or_else(|| AppError::Player(format!("a {VOICES}-voice mixer has no voice {index}")))
}

fn tone(level: f32, frames: usize, channels: u16) -> TestSource {
    TestSource::new(vec![level; frames * usize::from(channels)], channels, RATE)
}

/// **The sum does not clamp**, and a ceiling here would be invisible until a crossfade needed it.
/// Two voices under complementary linear ramps are inside unity by construction, so anything past
/// it is a bug upstream that has to stay audible rather than be quietly squashed.
#[test]
fn two_voices_sum_without_a_ceiling() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    for (index, level) in [0.8, 0.7].into_iter().enumerate() {
        voice_at(&mixer, index)?.append(tone(level, 16, 1));
    }

    let mut out = vec![0.0; 8];
    pull.fill(&mut out);

    assert!(out.iter().all(|s| (*s - 1.5).abs() < 1e-6), "the sum was clamped or scaled: {out:?}");
    Ok(())
}

#[test]
fn one_voice_alone_reaches_the_block_untouched() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    let input = [0.25, -0.5, 0.125, -0.0];
    voice_at(&mixer, 0)?.append(TestSource::new(input.to_vec(), 1, RATE));

    let mut out = vec![0.0; 4];
    pull.fill(&mut out);

    assert_eq!(bits(&out), bits(&input), "a lone voice at unity is not a passthrough");
    Ok(())
}

#[test]
fn a_paused_voice_contributes_nothing() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    voice_at(&mixer, 0)?.append(tone(0.5, 16, 1));

    let paused = voice_at(&mixer, 1)?;
    paused.append(tone(0.5, 16, 1));
    paused.pause();

    let mut out = vec![0.0; 8];
    pull.fill(&mut out);

    assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6), "{out:?}");
    Ok(())
}

#[test]
fn nothing_playing_reads_as_silence_rather_than_ending() {
    let (_mixer, mut pull) = pair(VOICES, device(2));

    let mut out = vec![1.0; 16];
    pull.fill(&mut out);

    assert!(out.iter().all(|s| *s == 0.0), "an idle mixer left the block alone: {out:?}");
}

/// The block is cleared before the voices add into it, or the previous callback's samples play
/// again wherever a voice has nothing to contribute.
#[test]
fn the_block_is_cleared_before_the_voices_add_into_it() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    voice_at(&mixer, 0)?.append(tone(0.25, 4, 1));

    let mut out = vec![9.0; 8];
    pull.fill(&mut out);

    assert!(out[..4].iter().all(|s| (*s - 0.25).abs() < 1e-6), "{out:?}");
    assert!(out[4..].iter().all(|s| *s == 0.0), "stale samples past the source: {out:?}");
    Ok(())
}

/// A block that is not a whole number of frames would shear the channel layout for everything after
/// it, so no voice writes the remainder — but it is still zeroed, because the caller stages into a
/// buffer it reuses and would otherwise pass the last block's samples through.
#[test]
fn a_partial_trailing_frame_is_silenced_rather_than_half_filled() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(2));
    voice_at(&mixer, 0)?.append(tone(0.5, 8, 2));

    let mut out = vec![9.0; 5];
    pull.fill(&mut out);

    assert!(out[..4].iter().all(|s| (*s - 0.5).abs() < 1e-6), "{out:?}");
    assert_eq!(bits(&out[4..]), bits(&[0.0]), "the odd sample kept a stale value: {out:?}");
    Ok(())
}

#[test]
fn a_voice_past_the_count_is_not_there() {
    let (mixer, _pull) = pair(VOICES, device(2));
    assert!(mixer.voice(VOICES).is_none());
}

/// A reopen reshapes the puller under a source that is part way through, and the next block has to
/// pick up at the frame the last one stopped on, laid out for the new device. A rebuild of anything
/// source-side would restart the track or drop the frames in hand.
#[test]
fn a_reshape_carries_on_from_the_same_source_frame() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    voice_at(&mixer, 0)?.append(TestSource::new(vec![0.5, 0.25, 0.125, -0.0625], 1, RATE));

    let mut before = vec![0.0; 2];
    pull.fill(&mut before);
    pull.reshape(device(2));
    let mut after = vec![0.0; 4];
    pull.fill(&mut after);

    assert_eq!(bits(&before), bits(&[0.5, 0.25]));
    assert_eq!(
        bits(&after),
        bits(&[0.125, 0.125, -0.0625, -0.0625]),
        "not continued at the new width"
    );
    Ok(())
}

/// The second voice renders into the sum's own buffer, sized per step at the device's width. A
/// reshape to more channels that left it at the old width would slice out of bounds on the audio
/// thread the first time two voices played together.
#[test]
fn a_reshape_to_more_channels_still_sums_two_voices() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    pull.reshape(device(2));
    for (index, level) in [0.5, 0.25].into_iter().enumerate() {
        voice_at(&mixer, index)?.append(tone(level, LOCKSTEP_FRAMES, 2));
    }

    let mut out = vec![0.0; LOCKSTEP_FRAMES * 2];
    pull.fill(&mut out);

    assert!(out.iter().all(|s| (*s - 0.75).abs() < 1e-6), "{out:?}");
    Ok(())
}

/// Frames numbered by position, so a block read back says exactly which source frames it carries.
fn counting(frames: usize) -> TestSource {
    #[expect(clippy::cast_precision_loss, reason = "a few hundred frames index exactly in an f32")]
    let data = (0..frames).map(|i| i as f32 / 1_000.0).collect();
    TestSource::new(data, 1, RATE)
}

/// The resync hold is silence *before* the track, not silence written over it: a source that
/// advanced through the hold would lose its head to the DAC's relock anyway, just somewhere else.
#[test]
fn a_hold_plays_silence_then_the_source_from_its_first_frame() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    let voice = voice_at(&mixer, 0)?;
    voice.append(counting(LOCKSTEP_FRAMES * 2));
    pull.hold(LOCKSTEP_FRAMES);

    let mut out = vec![9.0; LOCKSTEP_FRAMES * 2];
    pull.fill(&mut out);

    let (held, played) = out.split_at(LOCKSTEP_FRAMES);
    assert!(held.iter().all(|s| *s == 0.0), "the hold was not silent: {held:?}");
    let expected: Vec<f32> = counting(LOCKSTEP_FRAMES).collect();
    assert_eq!(bits(played), bits(&expected), "the source moved during the hold");
    Ok(())
}

#[test]
fn the_clock_stands_still_through_a_hold() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    let voice = voice_at(&mixer, 0)?;
    voice.append(counting(LOCKSTEP_FRAMES * 4));
    pull.hold(LOCKSTEP_FRAMES * 4);

    let mut out = vec![0.0; LOCKSTEP_FRAMES * 2];
    pull.fill(&mut out);

    assert_eq!(voice.position(), std::time::Duration::ZERO);
    Ok(())
}

/// `play_media` reopens and then clears the decks, and `clear` blocks until the callback services
/// it. A hold that skipped the voices would park that clear for its whole timeout and then leave
/// the old track loaded, which is what `is_empty` tells apart: the hold here outlasts the test.
#[test]
fn a_clear_issued_during_a_hold_still_lands() -> Result<(), AppError> {
    let (mixer, mut pull) = pair(VOICES, device(1));
    let voice = voice_at(&mixer, 0)?;
    voice.append(counting(LOCKSTEP_FRAMES));
    pull.hold(usize::MAX);

    let clearing = std::thread::spawn({
        let voice = Arc::clone(&voice);
        move || voice.clear()
    });
    let mut out = vec![0.0; LOCKSTEP_FRAMES];
    while !clearing.is_finished() {
        pull.fill(&mut out);
    }
    clearing.join().map_err(|_| AppError::Player("the clearing thread panicked".to_owned()))?;

    assert!(voice.is_empty(), "the clear was not serviced during the hold");
    Ok(())
}
