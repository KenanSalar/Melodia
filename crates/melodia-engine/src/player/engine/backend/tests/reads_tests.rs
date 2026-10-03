//! Tests for the settled position a long-pause release resumes from.
//!
//! The mixer has no device under it and only the test pulls it, so nothing sent to a deck is taken
//! until the test says.

use std::num::NonZero;
use std::path::Path;

use melodia_audio::player::source::audio::{Sample, Shape};
use melodia_core::error::AppError;
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::mixer::{self, MixerPull};
use melodia_playback::player::playback::replaygain::TrackReplayGain;
use melodia_testkit::ASSETS_DIR;

use super::PlaybackEngine;

const RATE: u32 = 44_100;
const CHANNELS: u16 = 2;

/// Where the state holds the paused track: past the end of the one-second fixture, so it can't be
/// mistaken for anything the deck reads.
const HELD_MS: u64 = 30_000;

/// How far the fixture plays before it is paused and sought.
const PLAYED_MS: u32 = 200;

/// Where the seek lands, inside the fixture and clear of [`PLAYED_MS`].
const SEEK_MS: u64 = 700;

/// An engine over a mixer the test pulls by hand.
fn engine() -> Result<(PlaybackEngine, MixerPull), AppError> {
    let device = Shape {
        channels: NonZero::new(CHANNELS).unwrap_or(NonZero::<u16>::MIN),
        rate: NonZero::new(RATE).unwrap_or(NonZero::<u32>::MIN),
    };
    let (mixer, pull) = mixer::pair(DECK_COUNT, device);
    Ok((PlaybackEngine::new(&mixer, tokio::runtime::Handle::current())?, pull))
}

/// One second of silence.
fn fixture() -> String {
    Path::new(ASSETS_DIR).join("silence.wav").to_string_lossy().into_owned()
}

fn play(engine: &PlaybackEngine) -> Result<(), AppError> {
    engine.play_media(&fixture(), 1.0, 1.0, None, TrackReplayGain::default())
}

/// Pull `ms` of audio, as the output callback would.
fn pull_ms(pull: &mut MixerPull, ms: u32) {
    let frames = usize::try_from(u64::from(RATE) * u64::from(ms) / 1_000).unwrap_or(0);
    let mut block: Vec<Sample> = vec![0.0; frames * usize::from(CHANNELS)];
    pull.fill(&mut block);
}

/// The fixture played for [`PLAYED_MS`], paused, and sought to [`SEEK_MS`], the seek's swap sent
/// and not yet taken.
fn sought_while_paused() -> Result<(PlaybackEngine, MixerPull), AppError> {
    let (engine, mut pull) = engine()?;
    play(&engine)?;
    pull_ms(&mut pull, PLAYED_MS);
    engine.pause();
    engine.seek(&fixture(), SEEK_MS, TrackReplayGain::default());
    Ok((engine, pull))
}

#[tokio::test]
async fn a_deck_yet_to_take_its_source_has_no_settled_position() -> Result<(), AppError> {
    let (engine, _pull) = engine()?;
    play(&engine)?;

    let settled = engine.query_settled_position(HELD_MS);

    assert_eq!(settled, None);
    Ok(())
}

/// Until the deck takes a seek's swap its clock reads where the seek left, which a release would
/// write back over where the seek went.
#[tokio::test]
async fn a_seek_the_deck_has_yet_to_take_has_no_settled_position() -> Result<(), AppError> {
    let (engine, _pull) = sought_while_paused()?;

    let settled = engine.query_settled_position(HELD_MS);

    assert_eq!(settled, None);
    Ok(())
}

/// Paused, the callback takes the swap and plays nothing, so the clock stands where it landed.
#[tokio::test]
async fn a_seek_the_deck_has_taken_settles_where_it_landed() -> Result<(), AppError> {
    let (engine, mut pull) = sought_while_paused()?;
    pull_ms(&mut pull, 10);

    let settled = engine.query_settled_position(HELD_MS);

    assert_eq!(settled, Some(SEEK_MS));
    Ok(())
}

/// A track that ran out under a pause fade leaves the deck empty and the device still held, its
/// clock at the end of the source. Answered with nothing, the release never gives the device back.
#[tokio::test]
async fn a_deck_whose_track_ran_out_answers_the_held_position() -> Result<(), AppError> {
    let (engine, mut pull) = engine()?;
    play(&engine)?;
    pull_ms(&mut pull, 1_500);

    let settled = engine.query_settled_position(HELD_MS);

    assert_eq!(settled, Some(HELD_MS));
    Ok(())
}

/// A clear zeroes the clock, and zero is no source's position: a second release over the deck the
/// first one emptied would otherwise write it back as where to resume.
#[tokio::test]
async fn a_deck_holding_nothing_answers_the_held_position_rather_than_its_zeroed_clock()
-> Result<(), AppError> {
    let (engine, _pull) = engine()?;

    let settled = engine.query_settled_position(HELD_MS);

    assert_eq!(settled, Some(HELD_MS));
    Ok(())
}
