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
use std::sync::Weak;

use melodia_audio::player::source::audio::Shape;
use melodia_audio::player::source::stream_source::{PreparedStream, prepared_stream_for_test};
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::mixer;

use melodia_audio::player::source::prebuffer::StreamShared;

use super::PlaybackEngine;
use melodia_core::error::AppError;

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

// --- Following the file's rate ---
//
// A fade needs the outgoing track playing across the transition, and a rate change reopens the
// output under it. So while the output follows the rate, every fade decision reads crossfade as
// off, automatic and manual alike, and hands the user's own choice back when following stops.

fn crossfade_all_on(engine: &PlaybackEngine) {
    engine.set_crossfade_enabled(true);
    engine.set_crossfade_manual(true);
    engine.set_crossfade_fade_on_pause(true);
}

#[tokio::test]
async fn following_the_rate_turns_every_track_change_fade_off() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    crossfade_all_on(&engine);
    engine.set_follow_rate(true);

    let settings = engine.crossfade_settings();
    assert!(!settings.enabled, "an automatic crossfade could cross a reopen");
    assert!(!settings.manual, "a manual crossfade could cross a reopen");
    Ok(())
}

/// A pause changes no track, so nothing reopens under its fade.
#[tokio::test]
async fn following_the_rate_keeps_the_fade_on_pause() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    crossfade_all_on(&engine);
    engine.set_follow_rate(true);

    assert!(engine.crossfade_settings().fade_on_pause);
    Ok(())
}

#[tokio::test]
async fn crossfade_comes_back_when_following_stops() -> Result<(), AppError> {
    let engine = engine_without_a_card()?;
    crossfade_all_on(&engine);
    engine.set_follow_rate(true);
    engine.set_follow_rate(false);

    let settings = engine.crossfade_settings();
    assert!(settings.enabled && settings.manual, "the user's own setting was overwritten");
    Ok(())
}
