use std::sync::Arc;

use crate::entities::track::TrackSummary;
use crate::player::state::{
    MAX_VOLUME, PlayerAction, PlayerState, RESTART_THRESHOLD_MS, play_track_inner,
    resume_from_stopped,
};
use crate::player::types::{PlaybackStatus, RepeatMode};

fn make_summary(id: i64, duration_ms: i64) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: format!("/music/{id}.mp3"),
        file_name: format!("{id}.mp3"),
        title: format!("Track {id}"),
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

fn state_with_queue(count: i64) -> PlayerState {
    let mut state = PlayerState::default();
    let tracks: Vec<_> = (1..=count).map(|i| make_summary(i, 180_000)).collect();
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);
    state
}

// --- play_track (direct play) ---

#[test]
fn play_track_sets_direct_play_and_actions() {
    let mut state = state_with_queue(3);
    let track = make_summary(99, 200_000);

    state.queue.set_direct_play(track.clone());
    let actions = play_track_inner(&mut state, track, None);

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(99));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { .. }))
    );
}

// --- play_tracks (queue replace + play) ---

#[test]
fn play_tracks_clears_queue_and_starts_at_index() {
    let mut state = PlayerState::default();
    let summaries: Vec<_> = (1..=5).map(|i| make_summary(i, 180_000)).collect();

    state.queue.clear();
    state.queue.add_tracks(summaries);
    state.queue.current_index = Some(2);
    state.queue.clear_direct_play();

    if let Some(track) = state.queue.get_current().cloned() {
        let actions = play_track_inner(&mut state, track, None);
        assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(3));
        assert!(!actions.is_empty());
    }
}

#[test]
fn play_tracks_start_index_clamps_to_len() {
    let summaries: Vec<_> = (1..=3).map(|i| make_summary(i, 180_000)).collect();
    let start = 100_usize;
    let clamped = start.min(summaries.len() - 1);
    assert_eq!(clamped, 2);
}

// --- play (resume) ---

#[test]
fn play_from_paused_resumes() {
    let mut state = state_with_queue(1);
    state.status = PlaybackStatus::Paused;

    let actions = state.build_play_actions();

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(actions, vec![PlayerAction::Resume]);
}

#[test]
fn play_from_stopped_resumes_from_stopped() {
    let mut state = state_with_queue(1);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.status = PlaybackStatus::Stopped;
    state.position_ms = 60_000;

    let actions = resume_from_stopped(&mut state);
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert!(!actions.is_empty());
}

// --- pause ---

#[test]
fn pause_when_playing_pauses() {
    let mut state = state_with_queue(1);
    state.status = PlaybackStatus::Playing;

    let actions = state.build_pause_actions();

    assert_eq!(state.status, PlaybackStatus::Paused);
    assert_eq!(actions, vec![PlayerAction::Pause]);
}

#[test]
fn pause_when_not_playing_noop() {
    let mut state = state_with_queue(1);
    state.status = PlaybackStatus::Stopped;

    let actions = state.build_pause_actions();

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.is_empty());
}

// --- stop ---

#[test]
fn stop_sets_stopped() {
    let mut state = state_with_queue(1);
    state.status = PlaybackStatus::Playing;

    let actions = state.build_stop_actions();

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(actions, vec![PlayerAction::Stop]);
}

// --- seek ---

#[test]
fn seek_updates_position() {
    let mut state = state_with_queue(1);
    state.position_ms = 0;

    let actions = state.build_seek_actions(45_000);

    assert_eq!(state.position_ms, 45_000);
    assert_eq!(
        actions,
        vec![PlayerAction::Seek {
            position_ms: 45_000,
        }]
    );
}

// --- next ---

#[test]
fn next_advances_queue() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 100_000; // > 50%

    let actions = state.build_next_actions();

    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { .. }))
    );
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, PlayerAction::UpdateSkipCount(_)))
    );
}

#[test]
fn next_tracks_skip_count_under_50pct() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 10_000;

    let actions = state.build_next_actions();

    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::UpdateSkipCount(1)))
    );
}

#[test]
fn next_no_skip_count_over_50pct() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 100_000;

    let actions = state.build_next_actions();

    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, PlayerAction::UpdateSkipCount(_)))
    );
}

#[test]
fn next_at_end_stops() {
    let mut state = state_with_queue(1);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);

    let actions = state.build_next_actions();

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::Stop)));
}

#[test]
fn next_while_paused_stays_paused() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.status = PlaybackStatus::Paused;

    let actions = state.build_next_actions();

    assert_eq!(state.status, PlaybackStatus::Paused);
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::Pause)));
}

// --- previous ---

#[test]
fn previous_restarts_after_threshold() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = RESTART_THRESHOLD_MS + 1;

    let actions = state.build_previous_actions();

    assert_eq!(state.position_ms, 0);
    assert_eq!(actions, vec![PlayerAction::Seek { position_ms: 0 }]);
}

#[test]
fn previous_goes_back_under_threshold() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 1000;

    assert!(state.position_ms <= RESTART_THRESHOLD_MS);
    let actions = state.build_previous_actions();

    assert!(!actions.is_empty());
}

// --- volume ---

#[test]
fn set_volume_clamps_and_unmutes() {
    let mut state = PlayerState {
        is_muted: true,
        ..Default::default()
    };

    let actions = state.build_set_volume_actions(999);

    assert_eq!(state.volume, MAX_VOLUME);
    assert!(!state.is_muted);
    assert_eq!(
        actions,
        vec![PlayerAction::SetVolume(state.effective_volume())]
    );
}

// --- mute ---

#[test]
fn set_muted_saves_pre_mute_volume() {
    let mut state = PlayerState {
        volume: 75,
        ..PlayerState::default()
    };

    let actions = state.build_set_muted_actions(true);

    assert_eq!(state.pre_mute_volume, 75);
    assert!(state.is_muted);
    assert!((state.effective_volume() - 0.0).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetVolume(0.0)]);
}

#[test]
fn toggle_mute_roundtrip() {
    let mut state = PlayerState {
        volume: 80,
        ..Default::default()
    };

    let actions = state.build_toggle_mute_actions();
    assert!(state.is_muted);
    assert!((state.effective_volume() - 0.0).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetVolume(0.0)]);

    let actions = state.build_toggle_mute_actions();
    assert!(!state.is_muted);
    let vol = state.effective_volume();
    assert!(vol > 0.0);
    assert_eq!(actions, vec![PlayerAction::SetVolume(vol)]);
}

// --- playback speed ---

#[test]
fn set_playback_speed_clamps_min() {
    let mut state = PlayerState::default();
    let actions = state.build_set_speed_actions(0.1);
    assert!((state.playback_speed - 0.25).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetSpeed(0.25)]);
}

#[test]
fn set_playback_speed_clamps_max() {
    let mut state = PlayerState::default();
    let actions = state.build_set_speed_actions(10.0);
    assert!((state.playback_speed - 2.0).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetSpeed(2.0)]);
}

#[test]
fn set_playback_speed_normal_value() {
    let mut state = PlayerState::default();
    let actions = state.build_set_speed_actions(1.5);
    assert!((state.playback_speed - 1.5).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetSpeed(1.5)]);
}

// --- mute edge cases ---

#[test]
fn set_muted_false_does_not_overwrite_pre_mute() {
    let mut state = PlayerState {
        volume: 80,
        ..PlayerState::default()
    };

    state.build_set_muted_actions(true);
    assert_eq!(state.pre_mute_volume, 80);

    state.build_set_muted_actions(false);

    assert!(!state.is_muted);
    assert_eq!(state.pre_mute_volume, 80);
}

// --- next with repeat all ---

#[test]
fn next_at_end_with_repeat_all_wraps() {
    let mut state = state_with_queue(3);
    state.queue.set_repeat_mode(RepeatMode::All);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);

    state.queue.advance_skip();
    state.queue.advance_skip();

    let next = state.queue.advance_skip();
    assert_eq!(next.map(|t| t.id), Some(1));
}

// --- previous edge cases ---

#[test]
fn previous_from_start_stays_at_current() {
    let mut state = state_with_queue(1);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 0;

    if state.position_ms <= RESTART_THRESHOLD_MS {
        let prev = state.queue.previous();
        assert_eq!(prev.map(|t| t.id), Some(1));
    }
}

// --- toggle_play_pause branching (mirrors `player_toggle_play_pause`) ---

fn toggle(state: &mut PlayerState) -> Vec<PlayerAction> {
    match state.status {
        PlaybackStatus::Playing | PlaybackStatus::Loading => state.build_pause_actions(),
        PlaybackStatus::Paused | PlaybackStatus::Stopped => state.build_play_actions(),
    }
}

#[test]
fn toggle_from_playing_pauses() {
    let mut state = state_with_queue(1);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    assert_eq!(state.status, PlaybackStatus::Playing);

    let actions = toggle(&mut state);
    assert_eq!(state.status, PlaybackStatus::Paused);
    assert!(matches!(actions.as_slice(), [PlayerAction::Pause]));
}

#[test]
fn toggle_from_paused_resumes() {
    let mut state = state_with_queue(1);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.build_pause_actions();
    assert_eq!(state.status, PlaybackStatus::Paused);

    let actions = toggle(&mut state);
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert!(matches!(actions.as_slice(), [PlayerAction::Resume]));
}

#[test]
fn toggle_from_stopped_with_current_track_resumes() {
    let mut state = state_with_queue(2);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.build_stop_actions();
    assert_eq!(state.status, PlaybackStatus::Stopped);

    let actions = toggle(&mut state);
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { .. }))
    );
}

#[test]
fn toggle_from_stopped_without_track_is_noop() {
    let mut state = state_with_queue(0);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(state.current_track.is_none());

    let actions = toggle(&mut state);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.is_empty());
}
