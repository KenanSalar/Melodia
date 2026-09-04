use std::sync::Arc;

use super::{needs_station_reopen, resolve_start_slot};
use melodia_core::entities::track::TrackSummary;
use melodia_engine::player::engine::state::{
    MAX_VOLUME, PlayerAction, PlayerState, RESTART_THRESHOLD_MS, play_track_inner,
    resume_from_stopped,
};
use melodia_engine::player::engine::types::{PlaybackStatus, RadioNowPlaying, RepeatMode};
use melodia_playback::player::playback::replaygain::TrackReplayGain;

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

// --- play_tracks (queue replace + play) ---

#[test]
fn play_tracks_clears_queue_and_starts_at_index() {
    let mut state = PlayerState::default();
    let summaries: Vec<_> = (1..=5).map(|i| make_summary(i, 180_000)).collect();

    state.queue.clear();
    state.queue.add_tracks(summaries);
    state.queue.current_index = Some(2);

    if let Some(track) = state.queue.get_current().cloned() {
        let actions = play_track_inner(&mut state, track, None);
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.current_track().map(|t| t.id), Some(3));
        assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
    }
}

// --- resolve_start_slot ---

#[test]
fn start_slot_follows_the_picked_track() {
    let ids = vec![1, 2, 3];
    let summaries: Vec<_> = (1..=3).map(|i| make_summary(i, 180_000)).collect();
    assert_eq!(resolve_start_slot(&ids, &summaries, Some(2)), Some(2));
}

/// The regression this exists for: a row deleted between the view's fetch and
/// the click drops out of `summaries`, shifting every slot behind it. Resolving
/// by index would start on the neighbour.
#[test]
fn start_slot_survives_a_track_missing_from_the_fetch() {
    let ids = vec![1, 2, 3, 4, 5];
    let summaries: Vec<_> = [1, 3, 4, 5].into_iter().map(|i| make_summary(i, 180_000)).collect();

    // The user clicked track 4, which the displayed list holds at index 3.
    // Track 2 vanished before the fetch, so slot 3 is now track 5 — taking
    // the index at face value would start playback one track late.
    let start = resolve_start_slot(&ids, &summaries, Some(3));
    assert_eq!(start, Some(2));
    assert_eq!(start.and_then(|i| summaries.get(i)).map(|t| t.id), Some(4));
}

#[test]
fn start_slot_is_none_when_the_picked_track_is_gone() {
    let ids = vec![1, 2, 3];
    let summaries: Vec<_> = [1, 3].into_iter().map(|i| make_summary(i, 180_000)).collect();
    assert_eq!(resolve_start_slot(&ids, &summaries, Some(1)), None);
}

#[test]
fn start_slot_is_none_for_an_out_of_range_index() {
    let ids = vec![1, 2, 3];
    let summaries: Vec<_> = (1..=3).map(|i| make_summary(i, 180_000)).collect();
    assert_eq!(resolve_start_slot(&ids, &summaries, Some(100)), None);
}

#[test]
fn start_slot_is_none_without_an_index() {
    let ids = vec![1, 2, 3];
    let summaries: Vec<_> = (1..=3).map(|i| make_summary(i, 180_000)).collect();
    assert_eq!(resolve_start_slot(&ids, &summaries, None), None);
}

/// Mirrors `player_play_tracks`' seeding when shuffle is already on: the
/// clicked track ends up playing and every other track is still queued
/// exactly once behind it.
#[test]
fn play_tracks_with_shuffle_on_anchors_the_clicked_track() {
    let mut state = PlayerState::default();
    let summaries: Vec<_> = (1..=8).map(|i| make_summary(i, 180_000)).collect();

    state.queue.shuffle_enabled = true;
    state.queue.clear();
    state.queue.add_tracks(summaries);
    state.queue.current_index = Some(5);
    state.queue.shuffle_play_order_in_place(&mut rand::rng(), /* anchor_to_current */ true);

    assert_eq!(state.queue.current_index, Some(0));
    assert_eq!(state.queue.get_current().map(|t| t.id), Some(6));

    let mut queued: Vec<i64> = state.queue.tracks_in_play_order().iter().map(|t| t.id).collect();
    queued.sort_unstable();
    assert_eq!(queued, (1..=8).collect::<Vec<_>>());
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

    // A user pause carries the ramp length when fade-on-pause is on; `player_pause`
    // resolves the setting, exactly as `player_stop` does for `build_stop_actions`.
    let actions = state.build_pause_actions(250);

    assert_eq!(state.status, PlaybackStatus::Paused);
    assert_eq!(actions, vec![PlayerAction::Pause { fade_ms: 250 }]);
}

#[test]
fn pause_when_not_playing_noop() {
    let mut state = state_with_queue(1);
    state.status = PlaybackStatus::Stopped;

    let actions = state.build_pause_actions(250);

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.is_empty());
}

// --- stop ---

#[test]
fn stop_sets_stopped() {
    let mut state = state_with_queue(1);
    state.status = PlaybackStatus::Playing;

    let actions = state.build_stop_actions(0);

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 0 }]);
}

#[test]
fn stop_forwards_the_pause_fade_length() {
    let mut state = state_with_queue(1);
    state.status = PlaybackStatus::Playing;

    let actions = state.build_stop_actions(250);

    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 250 }]);
}

// --- seek ---

#[test]
fn seek_updates_position() {
    let mut state = state_with_queue(1);
    // Seated, because the action carries the file the backend rebuilds to seek it.
    play_track_inner(&mut state, make_summary(1, 180_000), None);
    state.position_ms = 0;

    let actions = state.build_seek_actions(45_000);

    assert_eq!(state.position_ms, 45_000);
    assert_eq!(
        actions,
        vec![PlayerAction::Seek {
            position_ms: 45_000,
            file_path: "/music/1.mp3".to_owned(),
            replaygain: TrackReplayGain::default(),
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

    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
    assert!(!actions.iter().any(|a| matches!(a, PlayerAction::UpdateSkipCount(_))));
}

#[test]
fn next_tracks_skip_count_under_50pct() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 10_000;

    let actions = state.build_next_actions();

    assert!(actions.iter().any(|a| matches!(a, PlayerAction::UpdateSkipCount(1))));
}

#[test]
fn next_no_skip_count_over_50pct() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 100_000;

    let actions = state.build_next_actions();

    assert!(!actions.iter().any(|a| matches!(a, PlayerAction::UpdateSkipCount(_))));
}

#[test]
fn next_at_end_stops() {
    let mut state = state_with_queue(1);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);

    let actions = state.build_next_actions();

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::Stop { .. })));
}

#[test]
fn next_while_paused_stays_paused_without_fading() {
    let mut state = state_with_queue(3);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.status = PlaybackStatus::Paused;

    let actions = state.build_next_actions();

    assert_eq!(state.status, PlaybackStatus::Paused);
    // `fade_ms` MUST be 0. The `PlayMedia` ahead of this starts the deck, so a
    // fade here would ramp the incoming track down from full volume instead of
    // pausing it — its first quarter-second would be audible.
    assert!(
        actions.iter().any(|a| matches!(a, PlayerAction::Pause { fade_ms: 0 })),
        "next-while-paused must restore the pause without a fade, got {actions:?}"
    );
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
    assert_eq!(
        actions,
        vec![PlayerAction::Seek {
            position_ms: 0,
            file_path: "/music/1.mp3".to_owned(),
            replaygain: TrackReplayGain::default(),
        }]
    );
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
    assert_eq!(actions, vec![PlayerAction::SetVolume(state.effective_volume())]);
}

// --- mute ---

/// The unmute reads `pre_mute_volume`, so the mute edge has to save it and the unmute must
/// not overwrite it — a toggle-off that re-stamped it would pin the volume at zero.
#[test]
fn toggle_mute_roundtrip() {
    let mut state = PlayerState {
        volume: 80,
        ..Default::default()
    };

    let actions = state.build_toggle_mute_actions();
    assert!(state.is_muted);
    assert_eq!(state.pre_mute_volume, 80);
    assert!((state.effective_volume() - 0.0).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetVolume(0.0)]);

    let actions = state.build_toggle_mute_actions();
    assert!(!state.is_muted);
    assert_eq!(state.pre_mute_volume, 80);
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

// --- next with repeat all ---

#[test]
fn next_at_end_with_repeat_all_wraps() {
    let mut state = state_with_queue(3);
    // One cycle off the default is repeat-all, which is how the transport reaches it.
    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::All);
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
        PlaybackStatus::Playing | PlaybackStatus::Loading => state.build_pause_actions(250),
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
    assert!(matches!(actions.as_slice(), [PlayerAction::Pause { fade_ms: 250 }]));
}

#[test]
fn toggle_from_paused_resumes() {
    let mut state = state_with_queue(1);
    let track = make_summary(1, 180_000);
    play_track_inner(&mut state, track, None);
    state.build_pause_actions(250);
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
    state.build_stop_actions(0);
    assert_eq!(state.status, PlaybackStatus::Stopped);

    let actions = toggle(&mut state);
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
}

#[test]
fn toggle_from_stopped_without_track_is_noop() {
    let mut state = state_with_queue(0);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(state.current_track().is_none());

    let actions = toggle(&mut state);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.is_empty());
}

// --- radio transport routing -----------------------------------------------

fn station() -> std::sync::Arc<RadioNowPlaying> {
    melodia_engine::player::engine::fixtures::test_station("Example FM")
}

fn tuned_in() -> PlayerState {
    let mut state = state_with_queue(2);
    let (generation, _actions) = state.build_station_connecting_actions(station());
    let _started = state.build_station_connected_actions(generation);
    state
}

/// Pausing a station drops its connection, so the play half of a toggle is a fresh open rather
/// than a `Resume` — a network round trip the state machine cannot do under its lock. This
/// predicate is what routes it, shared with `player_play` so the two can't disagree.
#[test]
fn only_a_paused_station_routes_to_a_reopen() {
    assert!(needs_station_reopen(PlaybackStatus::Paused, true));
    assert!(!needs_station_reopen(PlaybackStatus::Playing, true), "already connected");
    assert!(!needs_station_reopen(PlaybackStatus::Loading, true), "already connecting");
    assert!(!needs_station_reopen(PlaybackStatus::Stopped, true), "a stop forgets the station");
    assert!(!needs_station_reopen(PlaybackStatus::Paused, false), "a paused track just resumes");
}

#[test]
fn toggling_a_playing_station_pauses_it_by_dropping_the_connection() {
    let mut state = tuned_in();
    assert!(!needs_station_reopen(state.status, state.station().is_some()));

    let actions = toggle(&mut state);

    assert_eq!(state.status, PlaybackStatus::Paused);
    assert!(matches!(actions.as_slice(), [PlayerAction::Stop { fade_ms: 250 }]));
    assert!(state.station().is_some(), "the station stays on screen");
    assert!(needs_station_reopen(state.status, state.station().is_some()), "and play re-opens it");
}

/// A connect that is still in flight is cancelled rather than resumed: the session generation moves,
/// so the stream it opens is refused when it arrives.
#[test]
fn toggling_a_connecting_station_cancels_the_connect() {
    let mut state = state_with_queue(2);
    let (generation, _actions) = state.build_station_connecting_actions(station());
    assert_eq!(state.status, PlaybackStatus::Loading);

    let actions = toggle(&mut state);

    assert_eq!(state.status, PlaybackStatus::Paused);
    assert!(matches!(actions.as_slice(), [PlayerAction::Stop { .. }]));
    assert_eq!(state.build_station_connected_actions(generation), vec![]);
}
