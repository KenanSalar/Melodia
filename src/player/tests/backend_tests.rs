//! The position conversions this module used to carry are gone with the two timelines that needed
//! them: a deck counts media frames, so there is nothing left to convert between.

use super::{PlaybackCheck, evaluate_playback_check};

// --- evaluate_playback_check ---

#[test]
fn gapless_transition_when_pending_and_queue_at_one() {
    assert_eq!(evaluate_playback_check(true, 1, false), PlaybackCheck::GaplessTransition);
}

#[test]
fn gapless_transition_when_pending_and_queue_empty() {
    assert_eq!(evaluate_playback_check(true, 0, true), PlaybackCheck::GaplessTransition);
}

#[test]
fn end_of_stream_when_empty_and_no_gapless() {
    assert_eq!(evaluate_playback_check(false, 0, true), PlaybackCheck::EndOfStream);
}

#[test]
fn playing_when_queue_has_sources() {
    assert_eq!(evaluate_playback_check(false, 2, false), PlaybackCheck::Playing);
}

#[test]
fn playing_when_gapless_pending_but_queue_still_two_deep() {
    assert_eq!(evaluate_playback_check(true, 2, false), PlaybackCheck::Playing);
}

#[test]
fn playing_when_single_source_no_gapless() {
    assert_eq!(evaluate_playback_check(false, 1, false), PlaybackCheck::Playing);
}
