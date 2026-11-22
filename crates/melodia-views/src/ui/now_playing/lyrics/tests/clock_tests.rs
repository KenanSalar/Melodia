//! Where the follow clock puts the song between reports. Wrong, a word lights ahead of the singing
//! and goes dark again, or the sheet holds still while the song moves on.

use std::time::{Duration, Instant};

use super::*;

const TRACK: i32 = 1;

/// Every instant here is a whole quarter second on, so positions are exact; this only absorbs the
/// float sum.
const TOLERANCE_MS: f64 = 0.001;

fn playing(position_ms: i32) -> Report {
    Report { track_id: TRACK, position_ms, playing: true, speed: 1.0 }
}

fn paused(position_ms: i32) -> Report {
    Report { playing: false, ..playing(position_ms) }
}

fn after(start: Instant, ms: u64) -> Instant {
    start + Duration::from_millis(ms)
}

/// A clock that took a report of 10 s at `start`, then ticked half a second on, showing 10.5 s:
/// what a later report is judged against.
fn showing_ten_and_a_half(start: Instant) -> SongClock {
    let clock = SongClock::new();
    clock.observe(playing(10_000), start);
    clock.observe(playing(10_000), after(start, 500));
    clock
}

fn assert_shows(clock: &SongClock, at: Instant, expected_ms: f64) {
    let shown = clock.position(at);
    assert!(
        (shown - expected_ms).abs() < TOLERANCE_MS,
        "showed {shown} ms, expected {expected_ms}"
    );
}

#[test]
fn between_reports_the_clock_runs_on_at_the_playback_speed() {
    let clock = SongClock::new();
    let start = Instant::now();

    clock.observe(Report { speed: 1.5, ..playing(10_000) }, start);

    assert_shows(&clock, after(start, 1_000), 11_500.0);
}

#[test]
fn a_paused_clock_stands_still() {
    let clock = SongClock::new();
    let start = Instant::now();

    clock.observe(paused(10_000), start);

    assert_shows(&clock, after(start, 5_000), 10_000.0);
}

#[test]
fn a_pause_freezes_the_clock_at_what_was_last_shown() {
    // Not where the song had got to by the pause: the words on screen would step on as it stopped.
    let start = Instant::now();
    let clock = showing_ten_and_a_half(start);

    clock.observe(paused(10_000), after(start, 1_000));

    assert_shows(&clock, after(start, 5_000), 10_500.0);
}

#[test]
fn a_report_behind_what_was_shown_holds_the_clock_where_it_was() {
    let start = Instant::now();
    let clock = showing_ten_and_a_half(start);

    clock.observe(playing(10_200), after(start, 500));

    assert_shows(&clock, after(start, 500), 10_500.0);
}

#[test]
fn a_held_clock_runs_on_from_the_report_once_the_song_catches_up() {
    let start = Instant::now();
    let clock = showing_ten_and_a_half(start);

    clock.observe(playing(10_200), after(start, 500));

    assert_shows(&clock, after(start, 1_500), 11_200.0);
}

#[test]
fn a_report_as_far_behind_as_the_slack_is_still_drift() {
    let start = Instant::now();
    let clock = showing_ten_and_a_half(start);

    clock.observe(playing(9_500), after(start, 500));

    assert_shows(&clock, after(start, 500), 10_500.0);
}

#[test]
fn a_report_a_millisecond_further_behind_than_the_slack_is_a_seek() {
    let start = Instant::now();
    let clock = showing_ten_and_a_half(start);

    clock.observe(playing(9_499), after(start, 500));

    assert_shows(&clock, after(start, 500), 9_499.0);
}

#[test]
fn a_report_for_another_track_is_taken_however_near_it_lands() {
    // Held as drift, the new track's first words would wait on the last one's position.
    let start = Instant::now();
    let clock = showing_ten_and_a_half(start);

    clock.observe(Report { track_id: TRACK + 1, ..playing(10_200) }, after(start, 500));

    assert_shows(&clock, after(start, 500), 10_200.0);
}

#[test]
fn a_report_onto_a_paused_clock_is_taken_as_it_stands() {
    let clock = SongClock::new();
    let start = Instant::now();
    clock.observe(paused(10_000), start);

    clock.observe(paused(9_800), after(start, 500));

    assert_shows(&clock, after(start, 500), 9_800.0);
}

#[test]
fn a_click_jumps_the_clock_back_to_its_line() {
    let clock = SongClock::new();
    let start = Instant::now();
    clock.observe(playing(30_000), start);

    clock.jump_to(10_000, after(start, 500));

    assert_shows(&clock, after(start, 500), 10_000.0);
}

#[test]
fn the_report_a_click_leaves_on_the_player_is_not_taken() {
    let clock = SongClock::new();
    let start = Instant::now();
    clock.observe(playing(30_000), start);
    clock.jump_to(10_000, after(start, 500));

    clock.observe(playing(30_000), after(start, 1_000));

    assert_shows(&clock, after(start, 1_000), 10_500.0);
}

#[test]
fn paused_a_click_then_play_counts_on_from_the_line_clicked() {
    // Found by hand: words lit ahead of the singing, then went dark.
    let clock = SongClock::new();
    let start = Instant::now();
    clock.observe(paused(30_000), start);
    clock.jump_to(10_000, after(start, 500));

    clock.observe(playing(30_000), after(start, 1_000));

    assert_shows(&clock, after(start, 1_500), 10_500.0);
}
