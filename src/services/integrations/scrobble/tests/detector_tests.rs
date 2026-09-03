use std::sync::Arc;

use super::{DetectorState, Effect};
use crate::entities::track::TrackSummary;
use crate::player::engine::state::{PlayerViewModelLight, PositionTick};

/// UNIX-seconds "now" at each play's start; every scrobble should carry it as its
/// timestamp.
const START: i64 = 1_000;

/// A minimal `TrackSummary` — only `id` and `duration_ms` matter to the detector.
fn summary(id: i64, duration_ms: i64) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: String::new(),
        file_name: String::new(),
        title: String::new(),
        artist: None,
        album: None,
        duration_ms,
        artwork_path: None,
        track_number: None,
        disc_number: None,
        last_position: 0,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    })
}

/// A view-model carrying `status` and an optional current track — the only two
/// things the detector reads.
fn vm(status: &'static str, current_track: Option<Arc<TrackSummary>>) -> PlayerViewModelLight {
    PlayerViewModelLight {
        status,
        current_track,
        position_ms: 0,
        duration_ms: 0,
        progress_percent: 0.0,
        volume: 100,
        is_muted: false,
        playback_speed: 1.0,
        gapless_enabled: false,
        sleep_at_track_end: false,
        radio: None,
        has_next: false,
        has_previous: false,
    }
}

fn tick(position_ms: u64, duration_ms: u64) -> PositionTick {
    PositionTick {
        position_ms,
        duration_ms,
    }
}

/// Feed position ticks from the play's start up to `target_ms` in
/// `SEEK_GUARD`-sized steps, so every delta counts as real playback and the
/// accumulator lands on `target_ms`.
fn play_to(d: &mut DetectorState, target_ms: u64, duration_ms: u64, now: i64) {
    let mut p = 0;
    while p < target_ms {
        p = (p + 5_000).min(target_ms);
        d.on_position(&tick(p, duration_ms), now);
    }
}

fn now_playing_count(effects: &[Effect]) -> usize {
    effects.iter().filter(|e| matches!(e, Effect::NowPlaying { .. })).count()
}

#[test]
fn track_start_now_plays_exactly_once() {
    let mut d = DetectorState::new();
    let started = d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    assert_eq!(started, vec![Effect::NowPlaying { track_id: 1 }]);
}

#[test]
fn normal_play_then_track_change_scrobbles() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 95_000, 180_000, START); // past the 90 s (half) threshold

    let changed = d.on_view_model(Some(&vm("playing", Some(summary(2, 200_000)))), 500_000);
    assert_eq!(
        changed,
        vec![
            Effect::Scrobble {
                track_id: 1,
                timestamp: START,
            },
            Effect::NowPlaying { track_id: 2 },
        ]
    );
}

#[test]
fn skip_before_threshold_does_not_scrobble() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 40_000, 180_000, START); // below 90 s

    let changed = d.on_view_model(Some(&vm("playing", Some(summary(2, 200_000)))), 100_000);
    assert_eq!(changed, vec![Effect::NowPlaying { track_id: 2 }]);
}

#[test]
fn skip_just_past_threshold_scrobbles() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 92_000, 180_000, START); // barely over 90 s

    let changed = d.on_view_model(Some(&vm("playing", Some(summary(2, 200_000)))), 100_000);
    assert_eq!(
        changed,
        vec![
            Effect::Scrobble {
                track_id: 1,
                timestamp: START,
            },
            Effect::NowPlaying { track_id: 2 },
        ]
    );
}

#[test]
fn a_seek_past_the_bar_is_not_counted_as_played_time() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 40_000, 180_000, START); // real play, below threshold
    // A forward seek near the end — a jump past SEEK_GUARD_MS must not accumulate.
    d.on_position(&tick(175_000, 180_000), START);

    let changed = d.on_view_model(Some(&vm("playing", Some(summary(2, 200_000)))), 300_000);
    assert_eq!(changed, vec![Effect::NowPlaying { track_id: 2 }]);
}

#[test]
fn a_paused_frozen_position_does_not_accumulate() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 85_000, 180_000, START); // just below 90 s
    // Fifty frozen-position ticks (a long pause) — zero deltas, no accumulation.
    for _ in 0..50 {
        d.on_position(&tick(85_000, 180_000), START);
    }

    let changed = d.on_view_model(Some(&vm("playing", Some(summary(2, 200_000)))), 500_000);
    assert_eq!(changed, vec![Effect::NowPlaying { track_id: 2 }]);
}

#[test]
fn a_restart_scrobbles_the_prior_play_and_now_plays_again() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 95_000, 180_000, START); // qualified

    let restart = d.on_position(&tick(0, 180_000), 300_000); // position dropped to 0
    assert_eq!(
        restart,
        vec![
            Effect::Scrobble {
                track_id: 1,
                timestamp: START,
            },
            Effect::NowPlaying { track_id: 1 },
        ]
    );

    // The accumulator reset: a short replay then stop must not scrobble again.
    play_to(&mut d, 10_000, 180_000, 300_000);
    let stopped = d.on_view_model(Some(&vm("stopped", None)), 400_000);
    assert_eq!(stopped, vec![]);
}

#[test]
fn a_restart_before_threshold_only_now_plays() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 20_000, 180_000, START); // below threshold

    let restart = d.on_position(&tick(0, 180_000), 300_000);
    assert_eq!(restart, vec![Effect::NowPlaying { track_id: 1 }]);
}

#[test]
fn republishes_of_the_same_track_now_play_only_once() {
    let mut d = DetectorState::new();
    let mut plays =
        now_playing_count(&d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START));
    // A pause, a volume-change republish, a loading blip, and advancing ticks —
    // all the same id, none should re-fire now-playing.
    plays +=
        now_playing_count(&d.on_view_model(Some(&vm("paused", Some(summary(1, 180_000)))), START));
    plays +=
        now_playing_count(&d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START));
    plays +=
        now_playing_count(&d.on_view_model(Some(&vm("loading", Some(summary(1, 180_000)))), START));
    plays += now_playing_count(&d.on_position(&tick(1_000, 180_000), START));
    plays += now_playing_count(&d.on_position(&tick(2_000, 180_000), START));
    assert_eq!(plays, 1);
}

#[test]
fn a_stop_finalizes_the_current_play() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 95_000, 180_000, START);

    let stopped = d.on_view_model(Some(&vm("stopped", Some(summary(1, 180_000)))), 300_000);
    assert_eq!(
        stopped,
        vec![Effect::Finalize {
            track_id: 1,
            timestamp: START,
        }]
    );
}

#[test]
fn shutdown_finalizes_the_current_play() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 180_000)))), START);
    play_to(&mut d, 95_000, 180_000, START);

    assert_eq!(
        d.on_shutdown(),
        vec![Effect::Finalize {
            track_id: 1,
            timestamp: START,
        }]
    );
}

#[test]
fn a_track_under_thirty_seconds_never_scrobbles() {
    let mut d = DetectorState::new();
    d.on_view_model(Some(&vm("playing", Some(summary(1, 20_000)))), START);
    play_to(&mut d, 20_000, 20_000, START); // played in full

    let changed = d.on_view_model(Some(&vm("playing", Some(summary(2, 200_000)))), 100_000);
    assert_eq!(changed, vec![Effect::NowPlaying { track_id: 2 }]);
}
