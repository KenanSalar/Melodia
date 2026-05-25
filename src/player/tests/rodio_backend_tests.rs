use std::time::Duration;

use super::{compute_position, evaluate_playback_check, PlaybackCheck};

// --- evaluate_playback_check ---

#[test]
fn gapless_transition_when_pending_and_queue_at_one() {
    assert_eq!(
        evaluate_playback_check(true, 1, false),
        PlaybackCheck::GaplessTransition
    );
}

#[test]
fn gapless_transition_when_pending_and_queue_empty() {
    assert_eq!(
        evaluate_playback_check(true, 0, true),
        PlaybackCheck::GaplessTransition
    );
}

#[test]
fn end_of_stream_when_empty_and_no_gapless() {
    assert_eq!(
        evaluate_playback_check(false, 0, true),
        PlaybackCheck::EndOfStream
    );
}

#[test]
fn playing_when_queue_has_sources() {
    assert_eq!(
        evaluate_playback_check(false, 2, false),
        PlaybackCheck::Playing
    );
}

#[test]
fn playing_when_gapless_pending_but_queue_still_two_deep() {
    assert_eq!(
        evaluate_playback_check(true, 2, false),
        PlaybackCheck::Playing
    );
}

#[test]
fn playing_when_single_source_no_gapless() {
    assert_eq!(
        evaluate_playback_check(false, 1, false),
        PlaybackCheck::Playing
    );
}

// --- compute_position ---

#[test]
fn position_at_normal_speed() {
    let wall = Duration::from_millis(5000);
    assert_eq!(compute_position(wall, 1.0), 5000);
}

#[test]
fn position_at_double_speed() {
    let wall = Duration::from_millis(5000);
    assert_eq!(compute_position(wall, 2.0), 10000);
}

#[test]
fn position_at_half_speed() {
    let wall = Duration::from_millis(5000);
    assert_eq!(compute_position(wall, 0.5), 2500);
}

#[test]
fn position_zero_wall_time() {
    assert_eq!(compute_position(Duration::ZERO, 1.0), 0);
}

#[test]
fn position_zero_speed() {
    let wall = Duration::from_millis(5000);
    assert_eq!(compute_position(wall, 0.0), 0);
}

#[test]
fn position_fractional_speed() {
    let wall = Duration::from_millis(10000);
    // 10000 * 0.75 = 7500
    assert_eq!(compute_position(wall, 0.75), 7500);
}

#[test]
fn position_large_wall_time() {
    let wall = Duration::from_secs(3600); // 1 hour
    assert_eq!(compute_position(wall, 1.0), 3_600_000);
}
