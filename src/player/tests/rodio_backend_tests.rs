use std::time::Duration;

use super::{compute_position, evaluate_playback_check, media_to_output_ms, PlaybackCheck};

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

// --- media_to_output_ms (inverse of compute_position) ---

#[test]
fn media_to_output_normal_speed_is_identity() {
    assert_eq!(media_to_output_ms(60_000, 1.0), 60_000);
}

#[test]
fn media_to_output_double_speed_halves() {
    // At 2× a media position of 60s sits at output time 30s.
    assert_eq!(media_to_output_ms(60_000, 2.0), 30_000);
}

#[test]
fn media_to_output_half_speed_doubles() {
    assert_eq!(media_to_output_ms(60_000, 0.5), 120_000);
}

#[test]
fn media_to_output_zero_speed_passes_through() {
    assert_eq!(media_to_output_ms(60_000, 0.0), 60_000);
}

#[test]
fn seek_round_trips_through_compute_position() {
    // Seeking to a media position and reading it back must be stable across
    // the supported speed range: compute_position(output, speed) == media.
    // This is the invariant that keeps the slider from jumping on a speed
    // change (set_speed re-anchors via seek_to_media).
    for &speed in &[0.25_f64, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
        let media_ms = 73_500_u64;
        let output_ms = media_to_output_ms(media_ms, speed);
        let read_back = compute_position(Duration::from_millis(output_ms), speed);
        // Allow ±1 ms for the two integer truncations in the round-trip.
        assert!(
            read_back.abs_diff(media_ms) <= 1,
            "speed {speed}: media {media_ms} -> output {output_ms} -> {read_back}"
        );
    }
}
