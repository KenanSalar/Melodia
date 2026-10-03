//! The position conversions this module used to carry are gone with the two timelines that needed
//! them: a deck counts media frames, so there is nothing left to convert between.

use super::{PlaybackCheck, evaluate_playback_check};

// --- evaluate_playback_check ---

#[test]
fn gapless_transition_when_pending_and_queue_at_one() {
    assert_eq!(evaluate_playback_check(true, 1), PlaybackCheck::GaplessTransition);
}

#[test]
fn gapless_transition_when_pending_and_queue_empty() {
    assert_eq!(evaluate_playback_check(true, 0), PlaybackCheck::GaplessTransition);
}

#[test]
fn end_of_stream_when_empty_and_no_gapless() {
    assert_eq!(evaluate_playback_check(false, 0), PlaybackCheck::EndOfStream);
}

#[test]
fn playing_when_queue_has_sources() {
    assert_eq!(evaluate_playback_check(false, 2), PlaybackCheck::Playing);
}

#[test]
fn playing_when_gapless_pending_but_queue_still_two_deep() {
    assert_eq!(evaluate_playback_check(true, 2), PlaybackCheck::Playing);
}

#[test]
fn playing_when_single_source_no_gapless() {
    assert_eq!(evaluate_playback_check(false, 1), PlaybackCheck::Playing);
}

// --- The staged stream, and the generation that owns it ---
//
// A station takes seconds to open and a click takes none, so an open that finishes late arrives
// for a session the user has already left. `radio_generation` is what refuses it, and all three
// doors here match on it. Every one of these failures costs a live connection or plays the wrong
// station, and none of them is visible from the outside until it happens.
//
// No deck is touched: staging and discarding never reach one, and `play_stream` returns on the
// refusal path before it does. The mixer is device-free and nothing pulls it.

use std::num::NonZero;
use std::path::Path;
use std::sync::Weak;

use melodia_audio::player::source::audio::Shape;
use melodia_audio::player::source::stream_source::{PreparedStream, prepared_stream_for_test};
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::mixer;

use melodia_audio::player::source::prebuffer::StreamShared;

use super::PlaybackEngine;
use melodia_core::error::AppError;
use melodia_testkit::ASSETS_DIR;

/// The session the tests treat as current. Any number does; two that differ is the whole subject.
const CURRENT: u64 = 7;
const ABANDONED: u64 = 3;

fn test_shape() -> Shape {
    Shape {
        channels: NonZero::new(2).unwrap_or(NonZero::<u16>::MIN),
        rate: NonZero::new(44_100).unwrap_or(NonZero::<u32>::MIN),
    }
}

/// An engine over a mixer with no device under it, which is all three doors need.
fn engine_without_a_card() -> Result<PlaybackEngine, AppError> {
    let (mixer, _pull) = mixer::pair(DECK_COUNT, test_shape());
    PlaybackEngine::new(&mixer, tokio::runtime::Handle::current())
}

fn staged_stream() -> (PreparedStream, Weak<StreamShared>) {
    prepared_stream_for_test(test_shape())
}

/// A discard naming a session that has already been superseded must leave the newer stage alone.
/// Taking it closes a connection the current session is waiting on, and the station never starts.
#[tokio::test]
async fn a_stale_discard_leaves_a_newer_session_s_stream_alone() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    let (prepared, watching) = staged_stream();
    engine.stage_stream(CURRENT, prepared);

    engine.discard_staged_stream(ABANDONED);

    assert!(watching.upgrade().is_some(), "the current session's connection was closed under it");
    Ok(())
}

/// The other side, and the reason the door exists: an open that finished after its session ended
/// owns a socket nobody will claim, and closing it here is what stops it outliving the station.
#[tokio::test]
async fn a_session_discarding_its_own_stage_closes_it() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    let (prepared, watching) = staged_stream();
    engine.stage_stream(CURRENT, prepared);

    engine.discard_staged_stream(CURRENT);

    assert!(watching.upgrade().is_none(), "an abandoned connection outlived its station");
    Ok(())
}

/// `play_stream` matches before it takes, which is a `take_if` and not a `take`. Taking first and
/// putting it back on a mismatch is the same code to read and drops the stage on the floor in
/// between, so the session it belonged to finds nothing when its own turn comes.
#[tokio::test]
async fn a_play_for_the_wrong_session_refuses_without_taking_the_stage() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    let (prepared, watching) = staged_stream();
    engine.stage_stream(CURRENT, prepared);

    let refused = engine.play_stream(ABANDONED, 1.0);

    assert!(matches!(refused, Err(AppError::Player(_))), "got {refused:?}");
    assert!(watching.upgrade().is_some(), "the refusal took the stage down with it");
    Ok(())
}

// --- What a track boundary decides from ---
//
// With no device under the mixer every source plays without a reopen, which leaves the probe's
// record and the crossfade settings as the only things these answers turn on.

/// Crossfade used to be switched off wholesale while the output followed each file's rate. Each
/// transition now asks about its own next track instead, so the user's settings pass through.
#[tokio::test]
async fn following_the_files_rate_leaves_the_crossfade_as_the_user_set_it() -> Result<(), AppError>
{
    let engine = engine_without_a_card()?;
    engine.set_crossfade_enabled(true);
    engine.set_follow_rate(true);

    let enabled = engine.crossfade_settings().enabled;

    assert!(enabled, "following the rate switched crossfade off");
    Ok(())
}

/// The monitor asks about the next track on every poll near the end of the current one, so what
/// opening its file found is kept rather than found again. Deleting the file after the first ask
/// shows it: a second open would fail, and a file that won't open reads as needing a reopen.
#[tokio::test]
async fn the_next_tracks_file_is_opened_once_for_every_ask_about_it() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    let dir = tempfile::tempdir()?;
    let next = dir.path().join("next.wav");
    std::fs::copy(Path::new(ASSETS_DIR).join("silence.wav"), &next)?;
    let first = engine.reopens_for(&next.to_string_lossy());
    std::fs::remove_file(&next)?;

    let second = engine.reopens_for(&next.to_string_lossy());

    assert_eq!((first, second), (false, false), "(before the delete, after it)");
    Ok(())
}

/// A file that won't open counts as one the output reopens for, so nothing fades into it.
#[tokio::test]
async fn a_next_track_that_wont_open_is_never_faded_into() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    let dir = tempfile::tempdir()?;

    let reopens = engine.reopens_for(&dir.path().join("gone.wav").to_string_lossy());

    assert!(reopens);
    Ok(())
}

/// A next track the output plays as it is comes back opened, ready to stage behind this one.
#[tokio::test]
async fn a_next_track_that_plays_as_it_is_is_handed_back_to_stage() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    let next = Path::new(ASSETS_DIR).join("silence.wav");

    let staged = engine.open_for_gapless(&next.to_string_lossy());

    assert!(matches!(staged, Ok(Some(_))));
    Ok(())
}

/// The preload asks on every tick until the track ends, so a file that wouldn't open is refused on
/// what the first ask found. Mending it in between shows it: a second open would now succeed.
#[tokio::test]
async fn a_next_track_that_wont_open_is_opened_once_for_every_gapless_ask() -> Result<(), AppError>
{
    let engine = engine_without_a_card()?;
    let dir = tempfile::tempdir()?;
    let next = dir.path().join("next.wav");
    std::fs::write(&next, b"not audio")?;
    let first = engine.open_for_gapless(&next.to_string_lossy());
    std::fs::copy(Path::new(ASSETS_DIR).join("silence.wav"), &next)?;

    let second = engine.open_for_gapless(&next.to_string_lossy());

    let asks = (first.is_err(), second.map(|decoded| decoded.is_some()).ok());
    assert_eq!(asks, (true, Some(false)), "(the first refused, the second staged)");
    Ok(())
}

/// The crossfade's ask and the preload's read one record of the next track, so the file is opened
/// once between them rather than once by each.
#[tokio::test]
async fn the_crossfade_and_the_preload_share_one_open_of_the_next_track() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    let dir = tempfile::tempdir()?;
    let next = dir.path().join("next.wav");
    std::fs::write(&next, b"not audio")?;
    let _reopens = engine.reopens_for(&next.to_string_lossy());
    std::fs::copy(Path::new(ASSETS_DIR).join("silence.wav"), &next)?;

    let staged = engine.open_for_gapless(&next.to_string_lossy());

    assert!(matches!(staged, Ok(None)), "the preload opened the file again");
    Ok(())
}
