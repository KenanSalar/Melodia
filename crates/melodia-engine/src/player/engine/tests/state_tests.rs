use std::sync::Arc;

use super::*;
use crate::player::engine::queue::current_index_from_i32;
use crate::player::engine::types::{PersistableQueue, PersistedPlayback, RepeatMode};
use melodia_core::error::AppError;
use melodia_playback::player::playback::crossfade::CrossfadeDecision;
use melodia_playback::player::playback::replaygain::TrackReplayGain;

/// Queue `tracks` with every record in the window, as the window task leaves a short queue.
fn queue_up(state: &mut PlayerState, tracks: Vec<Arc<TrackSummary>>) {
    let ids: Vec<i64> = tracks.iter().map(|track| track.id).collect();
    state.queue.add_tracks(&ids);
    state.window.seat(&ids, tracks);
}

/// Seat `persisted` as a launch does, `records` being its window's.
fn restore(state: &mut PlayerState, records: Vec<Arc<TrackSummary>>, persisted: &PersistableQueue) {
    let current = current_index_from_i32(persisted.current_index);
    restore_queue(state, QueueState::restored(persisted.track_ids.clone(), current), records);
}

fn make_summary(id: i64, title: &str, duration_ms: i64) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: format!("/music/{id}.mp3"),
        file_name: format!("{id}.mp3"),
        title: title.to_owned(),
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

#[test]
fn test_play_track_sets_state() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Test Song", 180_000);

    let _actions = play_track_inner(&mut state, track, None);

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.current_track().map(|t| t.id), Some(1));
    assert_eq!(state.position_ms, 0);
    assert_eq!(state.duration_ms, 180_000);
}

#[test]
fn test_play_track_with_start_position() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    let actions = play_track_inner(&mut state, track, Some(45_000));

    assert_eq!(state.position_ms, 45_000);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { start_position_ms: Some(45_000), .. }))
    );
}

#[test]
fn test_play_track_inner_with_resume_position() -> Result<(), AppError> {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    // Stopped part-way through, as a launch restores the last session's track
    play_track_inner(&mut state, track, None);
    state.position_ms = 60_000;
    state.status = PlaybackStatus::Stopped;

    // Resume: should replay from saved position
    let track = state
        .current_track()
        .cloned()
        .ok_or_else(|| AppError::validation_msg("current_track None"))?;
    let resume_pos = state.position_ms;
    let actions =
        play_track_inner(&mut state, track, if resume_pos > 0 { Some(resume_pos) } else { None });

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.position_ms, 60_000);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { start_position_ms: Some(60_000), .. }))
    );
    Ok(())
}

#[test]
fn test_resume_from_stopped_at_zero_starts_fresh() -> Result<(), AppError> {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    play_track_inner(&mut state, track, None);
    state.position_ms = 0;
    state.status = PlaybackStatus::Stopped;

    let track = state
        .current_track()
        .cloned()
        .ok_or_else(|| AppError::validation_msg("current_track None"))?;
    let resume_pos = state.position_ms;
    let actions =
        play_track_inner(&mut state, track, if resume_pos > 0 { Some(resume_pos) } else { None });

    assert_eq!(state.position_ms, 0);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { start_position_ms: None, .. }))
    );
    Ok(())
}

#[test]
fn test_play_track_start_position_clamped_to_near_end() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    // Position beyond duration — clamped to duration_ms - 500 to avoid immediate EOS
    let actions = play_track_inner(&mut state, track, Some(200_000));

    assert_eq!(state.position_ms, 179_500);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { start_position_ms: Some(179_500), .. }))
    );
}

#[test]
fn test_play_track_some_zero_filtered_to_none() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    // Some(0) is filtered to None — no unnecessary seek to position 0
    let actions = play_track_inner(&mut state, track, Some(0));

    assert_eq!(state.position_ms, 0);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { start_position_ms: None, .. }))
    );
}

#[test]
fn test_play_track_with_resume_does_not_eager_preload() {
    let mut state = PlayerState { gapless_enabled: true, ..Default::default() };

    let track1 = make_summary(1, "Song 1", 180_000);
    let track2 = make_summary(2, "Song 2", 180_000);
    queue_up(&mut state, vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);

    let actions = play_track_inner(&mut state, track1, Some(60_000));

    assert_eq!(state.position_ms, 60_000);
    // PlayMedia is issued; gapless preload is now staged late by the playback
    // monitor (see PRELOAD_LEAD_MS in `handlers.rs`), so it must NOT come from
    // play_track_inner — otherwise mid-track repeat-mode changes get clobbered.
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
    assert!(!actions.iter().any(|a| matches!(a, PlayerAction::PreloadGapless(_))));
}

#[test]
fn test_stop_end_of_queue_helper() {
    let mut state = PlayerState {
        status: PlaybackStatus::Playing,
        source: Some(PlaybackSource::Track(make_summary(1, "Song", 100_000))),
        position_ms: 50_000,
        ..Default::default()
    };

    let actions = stop_end_of_queue(&mut state);

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.position_ms, 0);
    assert!(state.current_track().is_some()); // Preserved for replay
    // Never faded — the track already ran out of audio.
    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 0 }]);
}

#[test]
fn test_play_track_start_position_clamp_on_short_track() {
    let mut state = PlayerState::default();
    // Track shorter than 500ms — saturating_sub clamps max_resume_pos to 0,
    // then filter(|&p| p > 0) converts Some(0) to None.
    let track = make_summary(1, "Short", 300);

    let actions = play_track_inner(&mut state, track, Some(250));

    assert_eq!(state.position_ms, 0);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { start_position_ms: None, .. }))
    );
}

#[test]
fn test_resume_from_stopped_helper() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);
    play_track_inner(&mut state, track, None);
    state.position_ms = 60_000;
    state.status = PlaybackStatus::Stopped;

    let actions = resume_from_stopped(&mut state);

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.position_ms, 60_000);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, PlayerAction::PlayMedia { start_position_ms: Some(60_000), .. }))
    );
}

#[test]
fn test_resume_from_stopped_no_track() {
    let mut state = PlayerState { status: PlaybackStatus::Stopped, ..Default::default() };

    let actions = resume_from_stopped(&mut state);
    assert!(actions.is_empty());
}

#[test]
fn test_resume_from_stopped_not_stopped() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);
    play_track_inner(&mut state, track, None);
    // Status is Playing, not Stopped
    let actions = resume_from_stopped(&mut state);
    assert!(actions.is_empty());
}

#[test]
fn test_stop_preserves_track() {
    let mut state = PlayerState {
        status: PlaybackStatus::Playing,
        source: Some(PlaybackSource::Track(make_summary(1, "Song", 100_000))),
        duration_ms: 100_000,
        position_ms: 50_000,
        ..Default::default()
    };

    state.build_stop_actions(0);

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.current_track().map(|t| t.id), Some(1));
    assert_eq!(state.duration_ms, 100_000);
}

#[test]
fn test_stop_at_end_of_queue_preserves_track() -> Result<(), AppError> {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "Song 1", 100_000);
    let track2 = make_summary(2, "Song 2", 100_000);

    queue_up(&mut state, vec![track1, track2]);

    let t = state.queue.advance().ok_or_else(|| AppError::validation_msg("advance None #1"))?;
    state.start_queued(t, false, 0);
    let t = state.queue.advance().ok_or_else(|| AppError::validation_msg("advance None #2"))?;
    state.start_queued(t, false, 0);

    // End of queue — no more tracks to advance to
    assert!(state.queue.advance().is_none());

    // Simulate end-of-queue stop (as in handlers.rs / player_next)
    state.status = PlaybackStatus::Stopped;
    state.position_ms = 0;

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert_eq!(state.position_ms, 0);
    Ok(())
}

#[test]
fn test_queue_add_and_remove() {
    let mut state = PlayerState::default();

    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
        make_summary(3, "Song 3", 100_000),
    ];

    queue_up(&mut state, tracks);
    assert_eq!(state.queue.play_order.len(), 3);

    state.queue.remove_at(1);
    assert_eq!(state.queue.play_order.len(), 2);
    // After removing index 1 (Song 2), play order should be [Song 1, Song 3]
    assert_eq!(*state.queue.play_ids(), [1, 3]);
}

#[test]
fn test_cycle_repeat_mode() {
    let mut state = PlayerState::default();

    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::All);

    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::One);

    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::Off);
}

#[test]
fn test_queue_loaded_restores_state() {
    let mut state = PlayerState::default();

    let tracks = vec![make_summary(1, "Song 1", 100_000), make_summary(2, "Song 2", 200_000)];

    queue_up(&mut state, tracks);
    state.queue.current_index = Some(1);
    state.queue.shuffle_enabled = false;
    state.queue.repeat_mode = RepeatMode::Off;

    if let Some(track) = state.queue.current_id().and_then(|id| state.record(id)) {
        state.duration_ms = u64::try_from(track.duration_ms).unwrap_or(0);
        state.source = Some(PlaybackSource::Track(track));
    }

    assert_eq!(state.queue.play_order.len(), 2);
    assert_eq!(state.queue.current_index, Some(1));
    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert_eq!(state.duration_ms, 200_000);
}

#[test]
fn test_view_model() {
    let state = PlayerState {
        status: PlaybackStatus::Playing,
        volume: 75,
        position_ms: 30_000,
        duration_ms: 120_000,
        ..Default::default()
    };

    let vm = state.to_view_model();

    assert_eq!(vm.status, "playing");
    assert_eq!(vm.volume, 75);
    assert_eq!(vm.position_ms, 30_000);
    assert_eq!(vm.duration_ms, 120_000);
    assert!((vm.progress_percent - 25.0).abs() < 0.01);
}

#[test]
fn test_position_tick() {
    let state = PlayerState { position_ms: 5000, duration_ms: 100_000, ..Default::default() };

    assert_eq!(state.position_ms, 5000);
    assert_eq!(state.duration_ms, 100_000);
}

#[test]
fn test_shuffle_unshuffle() {
    let mut state = PlayerState::default();

    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
        make_summary(3, "Song 3", 100_000),
    ];
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(1);

    // Apply shuffle
    state.queue.apply_shuffle_order(&[1, 2, 0]);
    assert!(state.queue.shuffle_enabled);
    assert_eq!(state.queue.current_index, Some(0));

    // Unshuffle
    state.queue.unshuffle();
    assert!(!state.queue.shuffle_enabled);
}

#[test]
fn test_volume_is_capped_at_max_and_converts_to_amplitude() {
    // The ceiling is enforced when the level is stored, not inside
    // `effective_volume`: a request above `MAX_VOLUME` clamps to it, so the
    // backend amplitude tops out at unity gain (no dead >100% band).
    let mut state = PlayerState::default();
    state.build_set_volume_actions(150);
    assert_eq!(state.volume, MAX_VOLUME);
    assert!((state.effective_volume() - 1.0).abs() < f64::EPSILON);

    state.build_set_volume_actions(50);
    assert_eq!(state.volume, 50);
    assert!((state.effective_volume() - 0.5).abs() < f64::EPSILON);

    state.is_muted = true;
    assert!((state.effective_volume() - 0.0).abs() < f64::EPSILON);
}

#[test]
fn test_play_track_inner_includes_speed() {
    let mut state = PlayerState { playback_speed: 1.5, ..Default::default() };

    let track = make_summary(1, "Song", 100_000);
    let actions = play_track_inner(&mut state, track, None);

    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia { speed, .. } if (*speed - 1.5).abs() < f64::EPSILON
    )));
}

// ── ViewModel tests ──

#[test]
fn test_to_view_model_empty_queue() {
    let state = PlayerState::default();
    let vm = state.to_view_model();

    assert_eq!(vm.status, "stopped");
    assert!(vm.current_track.is_none());
    assert_eq!(vm.queue_ids.len(), 0);
    assert_eq!(vm.queue_index, -1);
    assert!(!vm.has_next);
    assert!(!vm.has_previous);
    assert!((vm.progress_percent - 0.0).abs() < f64::EPSILON);
}

#[test]
fn test_to_view_model_has_previous_with_repeat_all() {
    let mut state = PlayerState::default();
    let tracks = vec![make_summary(1, "Song 1", 100_000), make_summary(2, "Song 2", 100_000)];
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(0);
    state.queue.repeat_mode = RepeatMode::All;

    let vm = state.to_view_model();
    assert!(vm.has_previous); // wraps around with RepeatMode::All
}

#[test]
fn test_to_view_model_has_previous_false_at_start() {
    let mut state = PlayerState::default();
    let tracks = vec![make_summary(1, "Song 1", 100_000), make_summary(2, "Song 2", 100_000)];
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(0);
    state.queue.repeat_mode = RepeatMode::Off;

    let vm = state.to_view_model();
    assert!(!vm.has_previous);
}

#[test]
fn test_to_view_model_zero_duration() {
    let state = PlayerState { duration_ms: 0, position_ms: 0, ..Default::default() };

    let vm = state.to_view_model();
    assert!((vm.progress_percent - 0.0).abs() < f64::EPSILON); // not NaN or Inf
}

#[test]
fn test_to_view_model_light_mirrors_full() {
    let mut state = PlayerState {
        status: PlaybackStatus::Playing,
        volume: 80,
        position_ms: 50_000,
        duration_ms: 200_000,
        ..Default::default()
    };

    let tracks = vec![make_summary(1, "Song 1", 200_000), make_summary(2, "Song 2", 100_000)];
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(0);
    state.source = Some(PlaybackSource::Track(make_summary(1, "Song 1", 200_000)));

    let full = state.to_view_model();
    let light = state.to_view_model_light();

    assert_eq!(full.status, light.status);
    assert_eq!(full.position_ms, light.position_ms);
    assert_eq!(full.duration_ms, light.duration_ms);
    assert_eq!(full.volume, light.volume);
    assert_eq!(full.is_muted, light.is_muted);
    assert!((full.playback_speed - light.playback_speed).abs() < f64::EPSILON);
    assert_eq!(full.gapless_enabled, light.gapless_enabled);
    assert_eq!(full.has_next, light.has_next);
    assert_eq!(full.has_previous, light.has_previous);
    assert!((full.progress_percent - light.progress_percent).abs() < f64::EPSILON);
}

/// The OS media panel reads both off the light model. Neither is the resting value, so a builder
/// that filled either with a default would fail here.
#[test]
fn the_light_view_model_carries_the_queues_shuffle_and_repeat() {
    let mut state = PlayerState::default();
    state.queue.shuffle_enabled = true;
    state.queue.repeat_mode = RepeatMode::One;

    let light = state.to_view_model_light();

    assert!(light.shuffle_enabled, "shuffle");
    assert_eq!(light.repeat_mode, RepeatMode::One, "repeat");
}

/// The other side of each, so a builder answering with the constants the test above sets still
/// fails somewhere.
#[test]
fn the_light_view_model_carries_shuffle_off_and_the_third_repeat_mode() {
    let mut state = PlayerState::default();
    state.queue.shuffle_enabled = false;
    state.queue.repeat_mode = RepeatMode::All;

    let light = state.to_view_model_light();

    assert!(!light.shuffle_enabled, "shuffle");
    assert_eq!(light.repeat_mode, RepeatMode::All, "repeat");
}

// ── QueueViewModel tests ──

#[test]
fn test_to_queue_view_model_correct_tracks() {
    let mut state = PlayerState::default();
    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
        make_summary(3, "Song 3", 100_000),
    ];
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(1);

    let qvm = state.to_queue_view_model();
    assert_eq!(qvm.ids.len(), 3);
    assert_eq!(qvm.queue_index, 1);
    assert!(!qvm.shuffle_enabled);
    assert_eq!(qvm.repeat_mode, RepeatMode::Off);
}

#[test]
fn test_to_queue_view_model_shuffled() {
    let mut state = PlayerState::default();
    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
        make_summary(3, "Song 3", 100_000),
    ];
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(1);
    state.queue.apply_shuffle_order(&[1, 2, 0]);

    let qvm = state.to_queue_view_model();
    assert!(qvm.shuffle_enabled);
    // Shuffled order: original indices [1, 2, 0] → tracks [2, 3, 1]
    assert_eq!(*qvm.ids, [2, 3, 1]);
}

#[test]
fn test_to_queue_view_model_has_next_at_end() {
    let mut state = PlayerState::default();
    let tracks = vec![make_summary(1, "Song 1", 100_000), make_summary(2, "Song 2", 100_000)];
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(1); // last track

    // RepeatMode::Off — no next
    state.queue.repeat_mode = RepeatMode::Off;
    let qvm = state.to_queue_view_model();
    assert!(!qvm.has_next);

    // RepeatMode::All — wraps, so has next
    state.queue.repeat_mode = RepeatMode::All;
    let qvm = state.to_queue_view_model();
    assert!(qvm.has_next);
}

// ── restore_queue tests ──

#[test]
fn test_restore_queue_basic() {
    let mut state = PlayerState::default();
    let tracks = vec![make_summary(1, "Song 1", 100_000), make_summary(2, "Song 2", 200_000)];
    let persistable = PersistableQueue { track_ids: vec![1, 2], current_index: 1 };

    restore(&mut state, tracks, &persistable);

    assert_eq!(state.queue.tracks.len(), 2);
    assert_eq!(state.queue.current_index, Some(1));
    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert_eq!(state.duration_ms, 200_000);
}

#[test]
fn test_restore_queue_empty() {
    let mut state = PlayerState::default();
    let persistable = PersistableQueue { track_ids: vec![], current_index: 0 };

    restore(&mut state, vec![], &persistable);

    assert!(state.current_track().is_none());
    assert_eq!(state.position_ms, 0);
    assert_eq!(state.duration_ms, 0);
}

#[test]
fn test_restore_queue_with_last_position() {
    let mut state = PlayerState::default();
    let track = Arc::new(TrackSummary {
        id: 1,
        file_path: "/music/1.mp3".to_owned(),
        file_name: "1.mp3".to_owned(),
        title: "Song 1".to_owned(),
        artist: None,
        album: None,
        duration_ms: 180_000,
        artwork_path: None,
        track_number: None,
        disc_number: None,
        last_position: 45_000,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    });

    let persistable = PersistableQueue { track_ids: vec![1], current_index: 0 };

    restore(&mut state, vec![track], &persistable);

    assert_eq!(state.position_ms, 45_000);
    assert_eq!(state.duration_ms, 180_000);
}

#[test]
fn test_restore_queue_index_out_of_bounds() {
    let mut state = PlayerState::default();
    let tracks = vec![make_summary(1, "Song 1", 100_000)];
    let persistable = PersistableQueue {
        track_ids: vec![1],
        current_index: 99, // way out of bounds
    };

    restore(&mut state, tracks, &persistable);

    // An index past the end names no track, so none is loaded and none is current.
    assert!(state.current_track().is_none());
    assert_eq!(state.queue.current_index, None);
}

#[test]
fn end_of_stream_advances_when_next_track_present() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "Song 1", 100_000);
    let track2 = make_summary(2, "Song 2", 100_000);
    state.source = Some(PlaybackSource::Track(track1.clone()));
    queue_up(&mut state, vec![track1, track2]);
    state.queue.current_index = Some(0);
    state.status = PlaybackStatus::Playing;

    let actions = state.build_end_of_stream_actions();

    // Advanced to track 2 and kept playing.
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert_eq!(state.queue.current_index, Some(1));
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
    // Counted the track that just ended.
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::UpdatePlayCount(1))));
}

#[test]
fn end_of_stream_stops_at_end_of_queue() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Only Song", 100_000);
    state.source = Some(PlaybackSource::Track(track.clone()));
    queue_up(&mut state, vec![track]);
    state.queue.current_index = Some(0);
    state.status = PlaybackStatus::Playing;

    let actions = state.build_end_of_stream_actions();

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.position_ms, 0);
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::Stop { .. })));
}

#[test]
fn end_of_stream_pauses_when_sleep_at_track_end_armed() {
    // ⚠ M6: with the sleep-timer "End of current track" flag armed, the
    // end-of-stream boundary must STOP (not advance) even though a next track
    // is queued, and disarm the flag so the following boundary advances again.
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "Song 1", 100_000);
    let track2 = make_summary(2, "Song 2", 100_000);
    state.source = Some(PlaybackSource::Track(track1.clone()));
    queue_up(&mut state, vec![track1, track2]);
    state.queue.current_index = Some(0);
    state.status = PlaybackStatus::Playing;
    state.pause_after_current_track = true;

    let actions = state.build_end_of_stream_actions();

    // Did NOT advance — current track unchanged, stopped at position 0.
    assert_eq!(state.current_track().map(|t| t.id), Some(1));
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.position_ms, 0);
    assert_eq!(state.queue.current_index, Some(0));
    // Flag disarmed so a subsequent boundary advances normally.
    assert!(!state.pause_after_current_track);
    // Emitted a Stop, still counted the finished track, started no new media.
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::Stop { .. })));
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::UpdatePlayCount(1))));
    assert!(!actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
}

#[test]
fn view_model_light_carries_sleep_at_track_end() {
    let mut state = PlayerState::default();
    assert!(!state.to_view_model_light().sleep_at_track_end);
    state.pause_after_current_track = true;
    assert!(state.to_view_model_light().sleep_at_track_end);
}

// --- crossfade -------------------------------------------------------------

/// The snapshot the monitor decided against. The staleness tests below build a
/// coherent one, then perturb the *state* to model a control op winning the race
/// between the decision and the emit lock.
fn decision(fade_ms: u64, track_id: i64, position_ms: u64) -> CrossfadeDecision {
    CrossfadeDecision { fade_ms, track_id: Some(track_id), position_ms }
}

#[test]
fn crossfade_advances_the_queue_and_starts_the_next_track() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    let track2 = make_summary(2, "Two", 200_000);
    queue_up(&mut state, vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.source = Some(PlaybackSource::Track(track1));
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 180_000;
    state.position_ms = 178_200;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    // State advances at fade *start* — Now-Playing switches as the overlap begins.
    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert_eq!(state.queue.current_index, Some(1));
    assert_eq!(state.position_ms, 0);
    assert_eq!(state.duration_ms, 200_000);
    assert_eq!(state.status, PlaybackStatus::Playing);

    // The outgoing track counts as played, then the incoming one starts.
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0], PlayerAction::UpdatePlayCount(1));
    let matched = matches!(
        &actions[1],
        PlayerAction::BeginCrossfade { file_path, fade_ms: 1_800, .. } if file_path == "/music/2.mp3"
    );
    assert!(matched, "expected BeginCrossfade for track 2, got {:?}", actions[1]);
}

#[test]
fn crossfade_emits_nothing_when_the_queue_has_no_next_track() {
    // The monitor's `peek_next` is read outside the emit lock, so a skip can
    // land in between. `advance()` returning None must leave state untouched.
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    queue_up(&mut state, vec![track1.clone()]);
    state.queue.current_index = Some(0);
    state.source = Some(PlaybackSource::Track(track1));
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 180_000;
    state.position_ms = 178_000;

    let actions = state.build_crossfade_actions(decision(2_000, 1, 178_000));

    assert!(actions.is_empty(), "no next track means no crossfade");
    assert_eq!(state.current_track().map(|t| t.id), Some(1));
    assert_eq!(
        state.status,
        PlaybackStatus::Playing,
        "the current track must keep playing to its own end"
    );
}

#[test]
fn crossfade_does_not_count_a_play_when_it_cannot_advance() {
    // Guards the ordering inside `build_crossfade_actions`: the play count is
    // only pushed once `advance()` has confirmed there is somewhere to go.
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    queue_up(&mut state, vec![track1.clone()]);
    state.queue.current_index = Some(0);
    state.source = Some(PlaybackSource::Track(track1));
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 180_000;
    state.position_ms = 178_000;

    let actions = state.build_crossfade_actions(decision(2_000, 1, 178_000));
    assert!(!actions.iter().any(|a| matches!(a, PlayerAction::UpdatePlayCount(_))));
}

/// The monitor decides to crossfade under the `PlayerState` lock but only
/// executes after taking `exec_lock`, so a pause can complete in between.
/// Forcing `Playing` back on would resurrect playback the user just stopped —
/// and `BeginCrossfade` would call `play()` on the deck, so it really would be
/// audible.
#[test]
fn crossfade_is_dropped_when_a_pause_landed_since_the_decision() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    let track2 = make_summary(2, "Two", 200_000);
    queue_up(&mut state, vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.source = Some(PlaybackSource::Track(track1));
    state.duration_ms = 180_000;
    state.position_ms = 178_200;
    // The pause won the race.
    state.status = PlaybackStatus::Paused;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    assert!(actions.is_empty(), "a paused player must not crossfade");
    assert_eq!(state.status, PlaybackStatus::Paused, "the pause must stick");
    assert_eq!(state.queue.current_index, Some(0), "the queue must not advance");
    assert_eq!(state.current_track().map(|t| t.id), Some(1));
}

/// Same window, but the user picked a different track instead of pausing.
/// Advancing here would skip straight past the track they just chose.
#[test]
fn crossfade_is_dropped_when_the_track_changed_since_the_decision() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    let track2 = make_summary(2, "Two", 200_000);
    let track3 = make_summary(3, "Three", 220_000);
    queue_up(&mut state, vec![track1, track2.clone(), track3]);
    // A manual jump to track 2 landed after the monitor decided on track 1.
    state.queue.current_index = Some(1);
    state.source = Some(PlaybackSource::Track(track2));
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 200_000;
    state.position_ms = 900;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    assert!(actions.is_empty(), "a stale decision must not crossfade");
    assert_eq!(
        state.queue.current_index,
        Some(1),
        "the track the user just picked must keep playing"
    );
    assert_eq!(state.current_track().map(|t| t.id), Some(2));
}

/// One case the track id alone misses: the *same* track restarted inside the
/// window. `play_track_inner` resets `position_ms` to 0, so the position is the
/// tell.
#[test]
fn crossfade_is_dropped_when_the_same_track_restarted_since_the_decision() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    let track2 = make_summary(2, "Two", 200_000);
    queue_up(&mut state, vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.source = Some(PlaybackSource::Track(track1));
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 180_000;
    state.position_ms = 0;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    assert!(actions.is_empty(), "a restarted track must not crossfade at position 0");
    assert_eq!(state.queue.current_index, Some(0));
}

/// The other case the id and the status both miss: a seek keeps the track and
/// keeps `Playing`, and moves only the position. Scrubbing backwards inside the
/// fade window would otherwise fade out and skip the track just scrubbed *into*.
#[test]
fn crossfade_is_dropped_when_a_seek_landed_since_the_decision() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    let track2 = make_summary(2, "Two", 200_000);
    queue_up(&mut state, vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.source = Some(PlaybackSource::Track(track1));
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 180_000;
    // The monitor decided at 178_200; the user scrubbed back to the middle.
    state.position_ms = 60_000;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    assert!(actions.is_empty(), "a seek must invalidate the pending crossfade");
    assert_eq!(
        state.queue.current_index,
        Some(0),
        "the track the user just scrubbed into must keep playing"
    );
    assert_eq!(state.position_ms, 60_000, "the seek must stick");
}

// ── seat_window_records ──

#[test]
fn seated_records_refresh_the_playing_track_and_the_window_and_republish() {
    use tokio::sync::watch;

    use crate::player::engine::event_sink::PlayerSinks;

    let handle = PlayerStateHandle::default();
    let (vm_tx, mut vm_rx) = watch::channel(None);
    let (q_tx, _q_rx) = watch::channel(None);
    let (awaited_start, _) = watch::channel(None);
    let sinks = PlayerSinks { view_model: vm_tx, queue: q_tx, media_controls: None, awaited_start };

    // Seed a current track + a coherent two-entry queue.
    with_state_emit(&handle, &sinks, |s| {
        let t1 = make_summary(1, "Old One", 1000);
        let t2 = make_summary(2, "Old Two", 2000);
        s.source = Some(PlaybackSource::Track(Arc::clone(&t1)));
        queue_up(s, vec![t1, t2]);
        s.queue.current_index = Some(0);
        Vec::<PlayerAction>::new()
    });
    drop(vm_rx.borrow_and_update());

    let fresh = vec![
        (*make_summary(1, "New One", 1000)).clone(),
        (*make_summary(2, "New Two", 2000)).clone(),
    ];
    seat_window_records(&handle, &sinks, fresh);

    {
        let g = lock_state(&handle);
        assert_eq!(
            g.current_track().map(|t| t.title.as_str()),
            Some("New One"),
            "the currently-playing summary must be refreshed"
        );
        assert_eq!(g.window.get(2).map(|t| t.title.as_str()), Some("New Two"));
    }
    assert!(
        matches!(vm_rx.has_changed(), Ok(true)),
        "the playing track's new tags must be published"
    );

    // An edit touching nothing queued/playing is a no-op: no publish.
    drop(vm_rx.borrow_and_update());
    seat_window_records(&handle, &sinks, vec![(*make_summary(99, "Nope", 1)).clone()]);
    assert!(
        matches!(vm_rx.has_changed(), Ok(false)),
        "an edit touching nothing playing must not republish"
    );
}

// ── with_state_emit ──

/// The view model and the position ticks reach the UI on two channels, and only the stamp says
/// which is newer. An emit that kept the stamp would lose to a tick read before it, putting the
/// position a stop or a seek moved off back on screen.
#[test]
fn every_emit_publishes_a_newer_position_stamp() {
    use tokio::sync::watch;

    use crate::player::engine::event_sink::PlayerSinks;

    let handle = PlayerStateHandle::default();
    let (vm_tx, vm_rx) = watch::channel(None);
    let (q_tx, _q_rx) = watch::channel(None);
    let (awaited_start, _) = watch::channel(None);
    let sinks = PlayerSinks { view_model: vm_tx, queue: q_tx, media_controls: None, awaited_start };
    let published = || vm_rx.borrow().as_ref().map(|vm| vm.position_stamp);

    with_state_emit(&handle, &sinks, |_| ());
    let first = published();
    with_state_emit(&handle, &sinks, |_| ());
    let second = published();

    assert!(second > first, "the second emit published {second:?} after {first:?}");
}

/// Every `PlayerAction` arm names the fact a reader needs from it.
///
/// A new variant is already a build failure — the match is exhaustive — so this
/// covers the silent half: an arm that drops its identifying field, or two whose
/// fields get swapped. Both compile, and are wrong only when read in a log.
#[test]
fn every_player_action_names_what_it_did() {
    let rg = TrackReplayGain::default();
    let cases: Vec<(PlayerAction, &[&str])> = vec![
        (
            PlayerAction::PlayMedia {
                file_path: "/music/a.flac".to_owned(),
                volume: 1.0,
                speed: 1.0,
                start_position_ms: Some(4_200),
                replaygain: rg,
            },
            &["/music/a.flac", "4200"],
        ),
        (
            PlayerAction::PlayMedia {
                file_path: "/music/b.flac".to_owned(),
                volume: 1.0,
                speed: 1.0,
                start_position_ms: None,
                replaygain: rg,
            },
            &["/music/b.flac"],
        ),
        (
            PlayerAction::BeginCrossfade {
                file_path: "/music/c.flac".to_owned(),
                replaygain: rg,
                fade_ms: 3_000,
                volume: 1.0,
                speed: 1.0,
            },
            &["/music/c.flac", "3000"],
        ),
        (PlayerAction::Resume, &["resume"]),
        (PlayerAction::Pause { fade_ms: 250 }, &["pause", "250"]),
        (PlayerAction::Stop { fade_ms: 0 }, &["stop", "0"]),
        (
            PlayerAction::Seek {
                position_ms: 9_001,
                file_path: "/music/e.flac".to_owned(),
                replaygain: TrackReplayGain::default(),
            },
            &["seek", "9001"],
        ),
        (PlayerAction::SetVolume(0.5), &["volume", "0.50"]),
        (PlayerAction::SetSpeed(1.25), &["speed", "1.25"]),
        (
            PlayerAction::PreloadGapless(Some("/music/d.flac".to_owned())),
            &["preload", "/music/d.flac"],
        ),
        (PlayerAction::PreloadGapless(None), &["clear"]),
        (PlayerAction::UpdatePlayCount(7), &["play count", "7"]),
        (PlayerAction::UpdateSkipCount(8), &["skip count", "8"]),
    ];

    for (action, expected) in cases {
        let rendered = action.to_string();
        for needle in expected {
            assert!(
                rendered.contains(needle),
                "{action:?} rendered as {rendered:?}, which is missing {needle:?}"
            );
        }
    }
}

// --- The transport builders ------------------------------------------------
//
// These came from `melodia-app`'s `library/tests/playback_tests.rs`, where they had grown up
// beside the doors that call them. The doors are worth a test of their own and now have one; the
// builders belong here, next to the machine they are part of. Several arrived over weaker
// namesakes that assigned a field and asserted it back, and those went.

/// A queue of `count` tracks sitting at the top, with nothing on the deck yet.
fn queued(count: i64) -> PlayerState {
    let mut state = PlayerState::default();
    let tracks: Vec<_> =
        (1..=count).map(|i| make_summary(i, &format!("Track {i}"), 180_000)).collect();
    queue_up(&mut state, tracks);
    state.queue.current_index = Some(0);
    state
}

#[test]
fn play_from_paused_resumes() {
    let mut state = queued(1);
    state.status = PlaybackStatus::Paused;

    let actions = state.build_play_actions();

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(actions, vec![PlayerAction::Resume]);
}

#[test]
fn play_from_stopped_resumes_from_stopped() {
    let mut state = queued(1);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);
    state.status = PlaybackStatus::Stopped;
    state.position_ms = 60_000;

    let actions = resume_from_stopped(&mut state);

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert!(!actions.is_empty());
}

#[test]
fn pause_when_playing_pauses() {
    let mut state = queued(1);
    state.status = PlaybackStatus::Playing;

    // A user pause carries the ramp length when fade-on-pause is on; `library::playback` resolves
    // the setting and hands it in, exactly as it does for `build_stop_actions`.
    let actions = state.build_pause_actions(250);

    assert_eq!(state.status, PlaybackStatus::Paused);
    assert_eq!(actions, vec![PlayerAction::Pause { fade_ms: 250 }]);
}

#[test]
fn pause_when_not_playing_noop() {
    let mut state = queued(1);
    state.status = PlaybackStatus::Stopped;

    let actions = state.build_pause_actions(250);

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.is_empty());
}

#[test]
fn stop_forwards_the_pause_fade_length() {
    let mut state = queued(1);
    state.status = PlaybackStatus::Playing;

    assert_eq!(state.build_stop_actions(0), vec![PlayerAction::Stop { fade_ms: 0 }]);

    state.status = PlaybackStatus::Playing;
    assert_eq!(state.build_stop_actions(250), vec![PlayerAction::Stop { fade_ms: 250 }]);
}

/// The stopped row is a launch, which restores the last session's track stopped part-way through
/// and still offers the hold.
#[test]
fn a_stop_takes_the_track_back_to_the_top_from_any_status() {
    for status in [PlaybackStatus::Playing, PlaybackStatus::Paused, PlaybackStatus::Stopped] {
        let mut state = PlayerState {
            status,
            source: Some(PlaybackSource::Track(make_summary(1, "Song", 180_000))),
            duration_ms: 180_000,
            position_ms: 60_000,
            ..Default::default()
        };

        state.build_stop_actions(0);

        assert_eq!((state.status, state.position_ms), (PlaybackStatus::Stopped, 0), "{status:?}");
    }
}

/// What a stop is for, as MPRIS specifies it. Taken from a pause, the one status that keeps a
/// position on purpose, so a stop that left it in place would show here as a resume.
#[test]
fn play_after_a_stop_starts_the_track_over() {
    let mut state = queued(1);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), Some(60_000));
    state.build_pause_actions(0);
    state.build_stop_actions(0);

    let actions = state.build_play_actions();

    assert!(
        matches!(actions.as_slice(), [PlayerAction::PlayMedia { start_position_ms: None, .. }]),
        "play after a stop must start from the top, not where the pause held it: {actions:?}"
    );
}

#[test]
fn seek_updates_position() {
    let mut state = queued(1);
    // Seated, because the action carries the file the backend rebuilds to seek it.
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);
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

#[test]
fn next_advances_queue() {
    let mut state = queued(3);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);
    state.position_ms = 100_000; // > 50%

    let actions = state.build_next_actions();

    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
}

/// Half the track is the line between a listen and a skip, and the skip count feeds Recently
/// Played and the stat-dependent smart playlists.
#[test]
fn next_counts_a_skip_only_under_half_the_track() {
    let mut under = queued(3);
    play_track_inner(&mut under, make_summary(1, "Track 1", 180_000), None);
    under.position_ms = 10_000;
    let actions = under.build_next_actions();
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::UpdateSkipCount(1))));

    let mut over = queued(3);
    play_track_inner(&mut over, make_summary(1, "Track 1", 180_000), None);
    over.position_ms = 100_000;
    let actions = over.build_next_actions();
    assert!(!actions.iter().any(|a| matches!(a, PlayerAction::UpdateSkipCount(_))));
}

#[test]
fn next_at_end_stops() {
    let mut state = queued(1);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);

    let actions = state.build_next_actions();

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::Stop { .. })));
}

#[test]
fn next_while_paused_stays_paused_without_fading() {
    let mut state = queued(3);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);
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

#[test]
fn previous_restarts_after_threshold() {
    let mut state = queued(3);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);
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
    let mut state = queued(3);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);
    state.queue.advance();
    play_track_inner(&mut state, make_summary(2, "Track 2", 180_000), None);
    state.position_ms = 1_000;

    let actions = state.build_previous_actions();

    assert_eq!(state.current_track().map(|t| t.id), Some(1), "under the threshold it steps back");
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
}

#[test]
fn set_volume_clamps_and_unmutes() {
    let mut state = PlayerState { is_muted: true, ..Default::default() };

    let actions = state.build_set_volume_actions(999);

    assert_eq!(state.volume, MAX_VOLUME);
    assert!(!state.is_muted, "moving the slider is how the user unmutes");
    assert_eq!(actions, vec![PlayerAction::SetVolume(state.effective_volume())]);

    state.build_set_volume_actions(75);
    assert_eq!(state.volume, 75, "a level inside the band passes through untouched");
}

/// The coarse and fine steps the keys and wheel send, on each end of the band and either side of
/// it, plus the delta's own extremes: a fast spin arrives as one summed delta, so nothing bounds it
/// to a single notch.
#[test]
fn a_step_lands_inside_the_volume_band() {
    let rows = [
        (50, 5, 55),
        (50, -5, 45),
        (50, 0, 50),
        (6, -5, 1),
        (5, -5, 0),
        (4, -5, 0),
        (0, -1, 0),
        (0, 1, 1),
        (MAX_VOLUME - 6, 5, MAX_VOLUME - 1),
        (MAX_VOLUME - 5, 5, MAX_VOLUME),
        (MAX_VOLUME - 4, 5, MAX_VOLUME),
        (MAX_VOLUME, 1, MAX_VOLUME),
        (MAX_VOLUME, -1, MAX_VOLUME - 1),
        (50, i32::MIN, 0),
        (50, i32::MAX, MAX_VOLUME),
    ];
    for (volume, delta, expected) in rows {
        let state = PlayerState { volume, ..Default::default() };

        let landed = state.stepped_volume(delta);

        assert_eq!(landed, expected, "{volume} stepped by {delta}");
    }
}

/// A muted player steps from the level it holds, not from the silence it plays.
#[test]
fn a_muted_step_starts_from_the_held_level_and_unmutes() {
    let mut state = PlayerState { volume: 40, is_muted: true, ..Default::default() };

    state.build_step_volume_actions(5);

    assert_eq!((state.volume, state.is_muted), (45, false));
}

#[test]
fn a_muted_step_sends_the_device_the_landed_level() {
    let mut state = PlayerState { volume: 40, is_muted: true, ..Default::default() };

    let actions = state.build_step_volume_actions(5);

    assert!(
        matches!(
            actions.as_slice(),
            [PlayerAction::SetVolume(amplitude)] if (*amplitude - 0.45).abs() < f64::EPSILON
        ),
        "the device must hear the level the step landed on, not the silence before it: {actions:?}"
    );
}

/// The row that bites is the held level while muted: setting it is how the user unmutes, so it
/// cannot count as held.
#[test]
fn a_volume_is_held_only_at_its_own_level_unmuted() {
    let rows = [
        (50, false, 50, true),
        (50, true, 50, false),
        (50, false, 55, false),
        (50, true, 55, false),
    ];
    for (volume, is_muted, level, expected) in rows {
        let state = PlayerState { volume, is_muted, ..Default::default() };

        let held = state.holds_volume(level);

        assert_eq!(held, expected, "{level} against {volume}, muted: {is_muted}");
    }
}

/// souvlaki states that a host may send a value outside `[0, 1]`, so the clamp is the whole of
/// what stands between an OS panel and a volume of 200. Half a percent is the other half:
/// rounding is what makes the quietest step reachable, where a truncating cast needs a full
/// percent before anything moves.
#[test]
fn the_volume_scale_clamps_its_input_and_rounds_its_output() {
    assert_eq!(amplitude_to_volume(-0.5), 0, "below the floor");
    assert_eq!(amplitude_to_volume(0.0), 0);
    assert_eq!(amplitude_to_volume(0.004), 0, "under half a percent");
    assert_eq!(amplitude_to_volume(0.005), 1, "and on it");
    assert_eq!(amplitude_to_volume(1.0), 100);
    assert_eq!(amplitude_to_volume(2.0), 100, "above the ceiling");
}

/// The unmute reads `pre_mute_volume`, so the mute edge has to save it and the unmute must
/// not overwrite it — a toggle-off that re-stamped it would pin the volume at zero.
#[test]
fn toggle_mute_roundtrip() {
    let mut state = PlayerState { volume: 80, ..Default::default() };

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

#[test]
fn set_playback_speed_clamps_to_its_band() {
    let mut state = PlayerState::default();

    let actions = state.build_set_speed_actions(0.1);
    assert!((state.playback_speed - 0.25).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetSpeed(0.25)]);

    let actions = state.build_set_speed_actions(10.0);
    assert!((state.playback_speed - 2.0).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetSpeed(2.0)]);

    let actions = state.build_set_speed_actions(1.5);
    assert!((state.playback_speed - 1.5).abs() < f64::EPSILON);
    assert_eq!(actions, vec![PlayerAction::SetSpeed(1.5)]);
}

#[test]
fn next_at_end_with_repeat_all_wraps() {
    let mut state = queued(3);
    // One cycle off the default is repeat-all, which is how the transport reaches it.
    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::All);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);

    state.queue.advance_skip();
    state.queue.advance_skip();

    assert_eq!(state.queue.advance_skip(), Some(1));
}

// --- The radio arm ---------------------------------------------------------
//
// A live stream is the one source with no track behind it, so every transport builder above has a
// branch for it. What these pin is that the branch is taken *and* that the queue underneath comes
// through untouched: stopping a station is supposed to hand the library back exactly as it was.

use crate::player::engine::fixtures::test_station as station;

/// A player mid-album, so every "the queue is untouched" assertion has something to be about.
fn playing_a_queue() -> PlayerState {
    let mut state = PlayerState::default();
    queue_up(&mut state, vec![make_summary(1, "One", 180_000), make_summary(2, "Two", 180_000)]);
    state.queue.current_index = Some(0);
    let track = make_summary(1, "One", 180_000);
    let _actions = play_track_inner(&mut state, track, Some(30_000));
    state
}

fn tuned_in() -> (PlayerState, u64) {
    let mut state = playing_a_queue();
    let (generation, _actions) = state.build_station_connecting_actions(station("Example FM"));
    (state, generation)
}

#[test]
fn connecting_to_a_station_clears_the_decks_and_leaves_the_queue_alone() {
    let mut state = playing_a_queue();
    let queue_before = state.queue.to_persistable();

    let (_generation, actions) = state.build_station_connecting_actions(station("Example FM"));

    assert_eq!(actions.first(), Some(&PlayerAction::Stop { fade_ms: 0 }));
    assert_eq!(state.status, PlaybackStatus::Loading);
    assert_eq!(state.station().map(|r| r.name.as_str()), Some("Example FM"));
    assert!(state.current_track().is_none(), "a station is not a track");
    assert_eq!(state.duration_ms, 0);
    assert_eq!(state.position_ms, 0);
    assert_eq!(state.queue.to_persistable(), queue_before, "the queue must survive verbatim");
}

/// Speed is a ratio on the deck's converter, so anything but 1.0 starves or overruns a
/// real-time source. Resetting the state alongside the deck is what keeps the transport honest.
#[test]
fn connecting_to_a_station_resets_playback_speed() {
    let mut state = playing_a_queue();
    let _speed = state.build_set_speed_actions(1.5);

    let (_generation, actions) = state.build_station_connecting_actions(station("Example FM"));

    assert!((state.playback_speed - 1.0).abs() < f64::EPSILON);
    assert!(actions.contains(&PlayerAction::SetSpeed(1.0)));
    // After the stop, so it lands on emptied decks and skips the re-anchoring seek.
    assert_eq!(actions.first(), Some(&PlayerAction::Stop { fade_ms: 0 }));
}

#[test]
fn a_station_already_at_unity_speed_emits_no_speed_action() {
    let mut state = playing_a_queue();

    let (_generation, actions) = state.build_station_connecting_actions(station("Example FM"));

    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 0 }]);
}

#[test]
fn a_connected_station_starts_the_staged_stream() {
    let (mut state, generation) = tuned_in();

    let actions = state.build_station_connected_actions(generation);

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(actions, vec![PlayerAction::PlayStream { generation, volume: 1.0 }]);
}

/// An open takes seconds and a click takes none, so a stream that arrives after the user moved on
/// must not start playing over whatever they moved to.
#[test]
fn a_stream_that_finished_connecting_too_late_is_refused() {
    let (mut state, stale) = tuned_in();
    let (fresh, _actions) = state.build_station_connecting_actions(station("Other FM"));
    assert_ne!(stale, fresh);

    assert_eq!(state.build_station_connected_actions(stale), vec![]);
    assert_eq!(state.status, PlaybackStatus::Loading, "the newer station keeps connecting");
    assert_eq!(state.station().map(|r| r.name.as_str()), Some("Other FM"));
}

/// [`tuned_in`] with the directory's word filled in, which is what a station tuned from a row
/// carries: the column names the container, so an Ogg mount is filed under `OGG` whichever codec
/// is inside it.
fn tuned_in_to_a_station_filed_under(codec: &str) -> (PlayerState, u64) {
    let mut filed = Arc::unwrap_or_clone(station("Example FM"));
    filed.codec = Some(codec.to_owned());

    let mut state = playing_a_queue();
    let (generation, _actions) = state.build_station_connecting_actions(Arc::new(filed));
    (state, generation)
}

/// The open is the first moment anything knows what an Ogg mount actually holds, and both answers
/// are kept: a user who filtered on `OGG` and landed on a page reading `OPUS` alone has nothing to
/// tell them those are the same station.
#[test]
fn a_measured_format_lands_on_the_station_already_on_the_bar() {
    let (mut state, generation) = tuned_in_to_a_station_filed_under("OGG");

    state.adopt_probed_codec(generation, "OPUS");

    assert_eq!(state.station().and_then(|r| r.codec.as_deref()), Some("OGG/OPUS"));
}

/// An open takes seconds and a click takes none, so a measurement arriving after the user moved on
/// must not rename whatever they moved to.
#[test]
fn a_format_measured_for_a_session_the_user_left_is_refused() {
    let (mut state, stale) = tuned_in_to_a_station_filed_under("OGG");
    let (_fresh, _actions) = state.build_station_connecting_actions(station("Other FM"));

    state.adopt_probed_codec(stale, "OPUS");

    assert_eq!(
        state.station().and_then(|r| r.codec.as_deref()),
        None,
        "the newer station was renamed"
    );
}

/// The bar already carries whatever the last play measured, so a second play composes against a
/// composition. The containment arms absorb a repeat, which is what made this look safe: it breaks
/// only for a station that changed what it serves, and then it names both codecs for one mount.
#[test]
fn a_second_play_of_the_same_station_does_not_name_its_format_twice() {
    let (mut state, generation) = tuned_in_to_a_station_filed_under("OGG");

    state.adopt_probed_codec(generation, "VORBIS");
    state.adopt_probed_codec(generation, "OPUS");

    assert_eq!(state.station().and_then(|r| r.codec.as_deref()), Some("OGG/OPUS"));
}

/// Observed as the allocation rather than the value, because the value is unchanged either way:
/// composing a measurement the bar already carries hands back what it was given, so a test reading
/// the string cannot see the guard go.
///
/// A second holder stands in for the published view model. `Arc::make_mut` copies only where the
/// handle is shared, which is exactly when the guard is worth having.
#[test]
fn a_measurement_that_repeats_what_the_bar_says_pays_for_no_copy() {
    let (mut state, generation) = tuned_in_to_a_station_filed_under("OGG");
    state.adopt_probed_codec(generation, "OPUS");
    let published = state.station().map(Arc::clone);

    state.adopt_probed_codec(generation, "OPUS");

    let held = published.as_ref().map(Arc::as_ptr);
    assert_eq!(
        held,
        state.station().map(Arc::as_ptr),
        "a repeated measurement rebuilt the station"
    );
}

/// A mount whose codec the decoder could not name answers nothing, and nothing is not a format.
#[test]
fn an_empty_measurement_is_not_a_format() {
    let (mut state, generation) = tuned_in_to_a_station_filed_under("OGG");

    state.adopt_probed_codec(generation, "");

    assert_eq!(state.station().and_then(|r| r.codec.as_deref()), Some("OGG"));
}

#[test]
fn a_failed_connect_forgets_the_station() {
    let (mut state, generation) = tuned_in();

    let actions = state.build_station_failed_actions(generation);

    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 0 }]);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(state.station().is_none());
}

#[test]
fn a_failure_report_from_a_superseded_session_is_ignored() {
    let (mut state, stale) = tuned_in();
    let (_fresh, _actions) = state.build_station_connecting_actions(station("Other FM"));

    assert_eq!(state.build_station_failed_actions(stale), vec![]);
    assert!(state.station().is_some(), "the newer station must survive the older one's failure");
}

/// S4: `stream-download` back-pressures its writer when the reader falls behind, so a held-open
/// socket would come back playing stale audio. Pause drops the connection and keeps the station.
#[test]
fn pausing_a_station_drops_the_connection_but_keeps_the_station() {
    let (mut state, generation) = tuned_in();
    let _started = state.build_station_connected_actions(generation);

    let actions = state.build_pause_actions(250);

    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 250 }]);
    assert_eq!(state.status, PlaybackStatus::Paused);
    assert!(state.station().is_some(), "the station stays on screen with a play button");
    assert_ne!(state.radio_generation, generation, "and its connection is invalidated");
}

#[test]
fn stopping_a_station_forgets_it_and_hands_the_queue_back() {
    let (mut state, generation) = tuned_in();
    let _started = state.build_station_connected_actions(generation);
    let queue_before = state.queue.to_persistable();

    let actions = state.build_stop_actions(0);

    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 0 }]);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(state.station().is_none());
    assert_eq!(state.queue.to_persistable(), queue_before);
}

/// The deck draining means the feed thread spent its reconnect budget. Advancing the queue there
/// would be a silent change of source, from a station to whatever the library was last on.
#[test]
fn a_station_going_off_air_stops_rather_than_advancing_the_queue() {
    let (mut state, generation) = tuned_in();
    let _started = state.build_station_connected_actions(generation);
    let queue_before = state.queue.to_persistable();

    let actions = state.build_end_of_stream_actions();

    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 0 }]);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(state.station().is_none());
    assert_eq!(state.queue.to_persistable(), queue_before, "the queue must not advance");
}

#[test]
fn the_transport_refuses_everything_a_live_source_cannot_do() {
    let (mut state, generation) = tuned_in();
    let _started = state.build_station_connected_actions(generation);
    let queue_before = state.queue.to_persistable();

    assert_eq!(state.build_next_actions(), vec![], "a station has no next");
    assert_eq!(state.build_previous_actions(), vec![], "a station has no previous");
    assert_eq!(state.build_seek_actions(30_000), vec![], "a station has no timeline");
    assert_eq!(state.build_set_speed_actions(1.5), vec![], "a station cannot be resampled");
    assert_eq!(state.build_play_actions(), vec![], "resuming a station is a fresh open");

    assert_eq!(state.position_ms, 0, "the refused seek must not move the position");
    assert!((state.playback_speed - 1.0).abs() < f64::EPSILON);
    assert_eq!(state.queue.to_persistable(), queue_before);
}

#[test]
fn a_station_reports_no_next_or_previous() {
    let (mut state, generation) = tuned_in();
    assert!(state.queue.peek_next().is_some(), "the queue underneath still has one");

    let vm = state.to_view_model_light();
    assert!(!vm.has_next);
    assert!(!vm.has_previous);

    let _started = state.build_station_connected_actions(generation);
    let vm = state.to_view_model_light();
    assert!(!vm.has_next);
    assert!(!vm.has_previous);
}

/// `source_allows` asks whether what is playing rules an action out, not whether something is
/// playing — so an empty deck permits everything. Spelling it `is_some_and` instead would leave a
/// fully loaded queue with its transport greyed out until the first track started, which is a
/// silent failure nothing else in the suite would catch.
#[test]
fn an_empty_deck_rules_nothing_out() {
    let mut state = PlayerState::default();
    queue_up(&mut state, vec![make_summary(1, "One", 100_000), make_summary(2, "Two", 100_000)]);
    state.queue.current_index = Some(0);
    assert!(state.current_track().is_none(), "nothing has reached the deck yet");

    for capability in [
        PlaybackSource::advances_queue,
        PlaybackSource::is_seekable,
        PlaybackSource::has_known_duration,
        PlaybackSource::has_variable_speed,
    ] {
        assert!(state.source_allows(capability));
    }

    let vm = state.to_view_model_light();
    assert!(vm.has_next, "a loaded queue offers Next before its first track plays");
}

#[test]
fn the_view_model_carries_the_station_instead_of_a_track() {
    let (mut state, generation) = tuned_in();
    let _started = state.build_station_connected_actions(generation);
    if let Some(radio) = state.station_mut() {
        let radio = std::sync::Arc::make_mut(radio);
        radio.live_title = Some("Artist - Track".to_owned());
        radio.buffering = true;
    }

    let vm = state.to_view_model_light();

    assert_eq!(vm.status, "playing", "a buffering station has not stopped");
    assert!(vm.current_track.is_none());
    assert_eq!(vm.duration_ms, 0);
    assert!((vm.progress_percent - 0.0).abs() < f64::EPSILON);
    let radio = vm.radio.as_ref();
    assert_eq!(radio.map(|r| r.name.as_str()), Some("Example FM"));
    assert_eq!(radio.and_then(|r| r.live_title.as_deref()), Some("Artist - Track"));
    assert_eq!(radio.map(|r| r.buffering), Some(true));
}

/// The one thing that must never reach the log: a stream URL can carry a session token, and this
/// line goes into the tail users attach to public issues.
#[test]
fn the_play_stream_action_never_renders_a_url() {
    let rendered = PlayerAction::PlayStream { generation: 7, volume: 1.0 }.to_string();

    assert!(rendered.contains('7'), "{rendered:?} should name the session it belongs to");
    assert!(!rendered.contains("http"), "{rendered:?} must not carry the stream URL");
}

/// A track and a station are one deck's worth of source, so starting one has to end the other.
/// Left standing, `radio` makes every transport builder below read the *track* as a live source,
/// and the session guard still passes for a connect the pick was supposed to have cancelled.
#[test]
fn starting_a_track_ends_the_station_it_replaces() {
    let (mut state, connecting) = tuned_in();

    let _actions = play_track_inner(&mut state, make_summary(2, "Two", 180_000), None);

    assert!(state.station().is_none(), "a track and a station cannot both be the source");
    assert_eq!(state.current_track().map(|t| t.id), Some(2));
    assert_eq!(
        state.build_station_connected_actions(connecting),
        vec![],
        "the connect still in flight must not start over the track that replaced it"
    );
}

/// The other half: every builder the radio arm refuses has to answer normally again.
#[test]
fn the_transport_is_a_tracks_again_once_one_replaces_the_station() {
    let (mut state, generation) = tuned_in();
    let _started = state.build_station_connected_actions(generation);

    let _actions = play_track_inner(&mut state, make_summary(1, "One", 180_000), None);

    let vm = state.to_view_model_light();
    assert!(vm.radio.is_none());
    assert!(vm.has_next, "the queue underneath is reachable again");

    assert_eq!(
        state.build_seek_actions(30_000),
        vec![PlayerAction::Seek {
            position_ms: 30_000,
            file_path: "/music/1.mp3".to_owned(),
            replaygain: TrackReplayGain::default(),
        }]
    );
    assert_eq!(state.build_pause_actions(250), vec![PlayerAction::Pause { fade_ms: 250 }]);
    assert_eq!(state.status, PlaybackStatus::Paused, "pausing a track is not a stop");
}

// --- The restart -----------------------------------------------------------
//
// `queue.json` is one file rather than two because a station leaves the queue seated underneath
// it, so a restart owes both back. The round trip below is the whole of that claim: everything
// above pins one side of it in memory, and nothing pinned it across the file.

/// A player mid-album with a station tuned over it, which is the state the two-halves claim is
/// about.
fn tuned_over_a_queue() -> PlayerState {
    let mut state = playing_a_queue();
    let (generation, _actions) = state.build_station_connecting_actions(station("Example FM"));
    let _started = state.build_station_connected_actions(generation);
    state
}

#[test]
fn a_station_over_a_queue_writes_both_halves_down() {
    let state = tuned_over_a_queue();

    let persisted = state.to_persisted();

    assert_eq!(persisted.queue.track_ids, vec![1, 2], "the queue underneath is still owed");
    assert_eq!(persisted.station_id, Some(42));
}

/// A station the user typed in but never saved has `station_id == 0`, and nothing could be looked
/// back up with it. Writing it down would restore a station with no row on the next boot.
#[test]
fn a_station_with_no_row_of_its_own_is_not_written_down() {
    let mut state = playing_a_queue();
    let unsaved = Arc::new(RadioNowPlaying { station_id: 0, ..(*station("Unsaved FM")).clone() });
    let (generation, _actions) = state.build_station_connecting_actions(unsaved);
    let _started = state.build_station_connected_actions(generation);

    assert!(state.station().is_some(), "the station is on the deck either way");
    assert_eq!(state.to_persisted().station_id, None);
}

/// Seating the station is what evicts whatever the queue restore just put on the deck, the two
/// being one field — but the queue itself has to survive that, or a stop hands back nothing.
#[test]
fn the_station_replaces_the_restored_track_and_not_the_queue_under_it() {
    let mut state = PlayerState::default();
    let persistable = PersistableQueue { track_ids: vec![1, 2], current_index: 0 };
    restore(
        &mut state,
        vec![make_summary(1, "One", 180_000), make_summary(2, "Two", 180_000)],
        &persistable,
    );
    assert!(state.current_track().is_some(), "the queue restore seats a track first");

    let _actions = restore_station(&mut state, station("Example FM"));

    assert!(state.current_track().is_none(), "a station is not a track");
    assert_eq!(state.station().map(|s| s.name.as_str()), Some("Example FM"));
    assert_eq!(state.queue.to_persistable(), persistable, "the queue must survive verbatim");
    assert_eq!(state.status, PlaybackStatus::Paused);
    assert_eq!(state.duration_ms, 0);
    assert_eq!(state.position_ms, 0);
}

/// The full trip through the file: what a session writes down is what the next one puts back,
/// both halves of it.
#[test]
fn a_restart_puts_the_queue_back_under_the_station() -> Result<(), AppError> {
    let before = tuned_over_a_queue();
    let written = serde_json::to_string(&before.to_persisted())
        .map_err(|e| AppError::validation("serialize queue.json", e))?;

    let read: PersistedPlayback =
        serde_json::from_str(&written).map_err(|e| AppError::validation("parse queue.json", e))?;
    let mut after = PlayerState::default();
    restore(
        &mut after,
        vec![make_summary(1, "One", 180_000), make_summary(2, "Two", 180_000)],
        &read.queue,
    );
    let _actions = restore_station(&mut after, station("Example FM"));

    assert_eq!(
        after.queue.to_persistable(),
        before.queue.to_persistable(),
        "the queue came back as it went down"
    );
    assert_eq!(after.station().map(|s| s.name.as_str()), Some("Example FM"));
    assert_eq!(read.station_id, Some(42), "and the id that found it survived the file");
    Ok(())
}

// --- a long pause giving an exclusive device back ---

/// The state's own position when the monitor decided to release, the last playing tick's.
const PAUSED_AT: u64 = 30_000;

/// Where the deck had stopped pulling by then, past [`PAUSED_AT`] by what the device held.
const STOPPED_PULLING_AT: u64 = 30_400;

/// Paused on track `id` at `position_ms`, as the monitor finds a long pause.
fn paused_on(id: i64, position_ms: u64) -> PlayerState {
    PlayerState {
        status: PlaybackStatus::Paused,
        source: Some(PlaybackSource::Track(make_summary(id, "Song", 180_000))),
        duration_ms: 180_000,
        position_ms,
        ..Default::default()
    }
}

/// What the monitor saw when it decided to release track 1.
fn release_decision() -> ReleaseDecision {
    ReleaseDecision { track_id: 1, position_ms: PAUSED_AT, resume_ms: STOPPED_PULLING_AT }
}

/// The track comes off the deck but stays paused, at the point the deck stopped pulling, since
/// everything before it has played out and play carries on from there.
#[test]
fn a_release_stops_the_deck_and_keeps_the_track_paused_where_it_stopped_pulling() {
    let mut state = paused_on(1, PAUSED_AT);

    let actions = state.build_release_actions(release_decision());

    assert_eq!(
        (actions, state.status, state.position_ms),
        (vec![PlayerAction::Stop { fade_ms: 0 }], PlaybackStatus::Paused, STOPPED_PULLING_AT)
    );
}

/// The monitor decides outside the emit lock, so anything that moved the pause in the gap turns
/// the release down: a play, another track, or a seek, the one thing that moves a paused position.
#[test]
fn a_release_is_refused_once_the_pause_it_was_decided_for_has_moved() {
    let rows = [
        (
            "playing again",
            PlayerState { status: PlaybackStatus::Playing, ..paused_on(1, PAUSED_AT) },
            PAUSED_AT,
        ),
        ("another track", paused_on(2, PAUSED_AT), PAUSED_AT),
        ("a seek in the gap", paused_on(1, 45_000), 45_000),
    ];
    for (what, mut state, position_ms) in rows {
        let actions = state.build_release_actions(release_decision());

        assert_eq!((actions, state.position_ms), (Vec::new(), position_ms), "{what}");
    }
}

/// A released track has nothing on the deck to resume, so play starts it again from its position,
/// reclaiming the device on the way. One paused at its very start starts fresh.
#[test]
fn a_replay_starts_the_paused_track_again_from_its_position() {
    let rows = [(STOPPED_PULLING_AT, Some(STOPPED_PULLING_AT)), (0, None)];
    for (position_ms, expected_start) in rows {
        let mut state = paused_on(1, position_ms);

        let actions = state.build_replay_actions();

        let started = actions.iter().find_map(|action| match action {
            PlayerAction::PlayMedia { start_position_ms, .. } => Some(*start_position_ms),
            _ => None,
        });
        assert_eq!(
            (started, state.status),
            (Some(expected_start), PlaybackStatus::Playing),
            "paused at {position_ms} ms"
        );
    }
}

/// Only a paused track can have been released, so a replay asked of any other state does nothing.
#[test]
fn a_replay_is_refused_unless_the_track_is_paused() {
    for status in [PlaybackStatus::Playing, PlaybackStatus::Stopped, PlaybackStatus::Loading] {
        let mut state = PlayerState { status, ..paused_on(1, PAUSED_AT) };

        let actions = state.build_replay_actions();

        assert_eq!((actions, state.status), (Vec::new(), status), "{status:?}");
    }
}

// --- A transition onto a track the window lacks ---
//
// The transport runs under the state lock, where nothing may wait on the database, so a
// transition landing on a track whose record the window doesn't hold moves the queue and owes the
// start. The window task fetches the record and runs it.

use std::collections::HashSet;

/// A transition a row runs, and the actions it builds.
type Transition = fn(&mut PlayerState) -> Vec<PlayerAction>;

/// What a row lands on the state while a start is owed.
type StateEdit = fn(&mut PlayerState);

/// What a row does to the queue while a start is owed.
type QueueEdit = fn(&mut QueueState);

/// [`queued`] playing its first track past half, so a Next counts no skip.
fn playing_first_of(count: i64) -> PlayerState {
    let mut state = queued(count);
    play_track_inner(&mut state, make_summary(1, "Track 1", 180_000), None);
    state.position_ms = 120_000;
    state
}

/// [`queued`] at its second slot, playing track 2 from the top.
fn playing_second_of(count: i64) -> PlayerState {
    let mut state = queued(count);
    state.queue.advance();
    play_track_inner(&mut state, make_summary(2, "Track 2", 180_000), None);
    state
}

/// `state` with its window holding the records of `held` and no others.
fn holding(mut state: PlayerState, held: &[i64]) -> PlayerState {
    state.window.seat(held, std::iter::empty());
    state
}

/// The start `actions` owe, where they owe one.
fn owed_start(actions: &[PlayerAction]) -> Option<AwaitedStart> {
    actions.iter().find_map(|action| match action {
        PlayerAction::StartWhenResolved(owed) => Some(*owed),
        _ => None,
    })
}

/// The file `actions` start on the deck, where they start one.
fn started_path(actions: &[PlayerAction]) -> Option<&str> {
    actions.iter().find_map(|action| match action {
        PlayerAction::PlayMedia { file_path, .. } => Some(file_path.as_str()),
        _ => None,
    })
}

/// A queue of `count` after a Next onto track 2, whose record the window lacked, and the start
/// that Next owes.
fn awaiting_track_two(count: i64) -> Result<(PlayerState, AwaitedStart), AppError> {
    let mut state = holding(playing_first_of(count), &[1]);
    let actions = state.build_next_actions();
    let owed = owed_start(&actions)
        .ok_or_else(|| AppError::validation_msg(format!("the Next owed no start: {actions:?}")))?;
    Ok((state, owed))
}

/// The window task landing the records of `ids`, of those the window still wants.
fn land_records(state: &mut PlayerState, ids: &[i64]) {
    let wanted = state.queue.window_ids();
    let records = ids.iter().map(|&id| make_summary(id, &format!("Track {id}"), 180_000));
    state.window.seat(&wanted, records);
}

/// The Stop is what takes the outgoing track off the deck. Without it the old track plays on under
/// a state that says the new one is loading.
#[test]
fn a_skip_onto_a_track_the_window_lacks_stops_the_deck_and_owes_its_start() {
    let mut state = holding(playing_first_of(3), &[1]);

    let actions = state.build_next_actions();

    assert_eq!(
        actions,
        vec![
            PlayerAction::Stop { fade_ms: 0 },
            PlayerAction::StartWhenResolved(AwaitedStart { id: 2, re_pause: false, skipped: 0 }),
        ]
    );
}

/// `Loading` over an empty deck is what keeps the monitor from ticking while the record is
/// fetched, and from ending a drained stream a second time. The queue has already moved, so the
/// sheet and Up Next show where playback is going.
#[test]
fn a_deferred_start_leaves_nothing_on_the_deck_under_loading() {
    let mut state = holding(playing_first_of(3), &[1]);

    state.build_next_actions();

    assert_eq!(
        (
            state.status,
            state.source.is_none(),
            state.position_ms,
            state.duration_ms,
            state.queue.current_index
        ),
        (PlaybackStatus::Loading, true, 0, 0, Some(1)),
    );
}

/// Every way playback moves along the queue reads the window, so each one has to owe its start
/// on a miss rather than stopping or starting a record it doesn't hold.
#[test]
fn every_transition_onto_a_track_the_window_lacks_owes_that_tracks_start() {
    let rows: [(&str, PlayerState, Transition, i64); 5] = [
        ("Next", holding(playing_first_of(3), &[1]), PlayerState::build_next_actions, 2),
        ("Previous", holding(playing_second_of(3), &[2]), PlayerState::build_previous_actions, 1),
        (
            "an end of stream",
            holding(playing_first_of(3), &[1]),
            PlayerState::build_end_of_stream_actions,
            2,
        ),
        (
            "Play with nothing on the deck",
            holding(queued(3), &[]),
            PlayerState::build_play_actions,
            1,
        ),
        (
            "a gapless handover the queue moved after the stage",
            holding(playing_first_of(3), &[1, 3]),
            |s| s.build_gapless_advance_actions(Some(make_summary(3, "Track 3", 180_000))),
            2,
        ),
    ];
    for (what, mut state, transition, id) in rows {
        let actions = transition(&mut state);

        assert_eq!(
            owed_start(&actions),
            Some(AwaitedStart { id, re_pause: false, skipped: 0 }),
            "{what}"
        );
    }
}

/// A skip made while paused stays paused across the fetch: the start puts the track back on pause
/// once it runs, unfaded for the reason `restore_paused` gives.
#[test]
fn a_skip_made_while_paused_comes_back_paused_once_its_record_lands() -> Result<(), AppError> {
    let mut state = holding(playing_first_of(3), &[1]);
    state.status = PlaybackStatus::Paused;
    let owed = owed_start(&state.build_next_actions())
        .ok_or_else(|| AppError::validation_msg("the paused Next owed no start"))?;
    land_records(&mut state, &[2]);

    let actions = state.resume_awaited(owed);

    assert_eq!(
        (state.status, actions.last()),
        (PlaybackStatus::Paused, Some(&PlayerAction::Pause { fade_ms: 0 }))
    );
    Ok(())
}

#[test]
fn a_deferred_start_runs_once_the_window_holds_its_record() -> Result<(), AppError> {
    let (mut state, owed) = awaiting_track_two(3)?;
    land_records(&mut state, &[2]);

    let actions = state.resume_awaited(owed);

    assert_eq!(started_path(&actions), Some("/music/2.mp3"));
    Ok(())
}

/// A deferred start runs only while nothing has started or stopped since it was owed. Run after a
/// Stop it plays over the stop, after a Next it cuts off the track that Next started, and over a
/// station connecting it puts a track on the station's deck, `Loading` though the state still is.
#[test]
fn a_deferred_start_is_refused_once_something_else_started_or_stopped() -> Result<(), AppError> {
    let rows: [(&str, StateEdit, PlaybackStatus); 3] = [
        (
            "a Stop",
            |s| {
                s.build_stop_actions(0);
            },
            PlaybackStatus::Stopped,
        ),
        (
            "a Next that started its track",
            |s| {
                land_records(s, &[3]);
                s.build_next_actions();
            },
            PlaybackStatus::Playing,
        ),
        (
            "a station connecting",
            |s| {
                s.build_station_connecting_actions(station("Example FM"));
            },
            PlaybackStatus::Loading,
        ),
    ];
    for (what, intervene, status) in rows {
        let (mut state, owed) = awaiting_track_two(3)?;
        intervene(&mut state);
        land_records(&mut state, &[2]);

        let actions = state.resume_awaited(owed);

        assert_eq!((actions, state.status), (Vec::new(), status), "{what}");
    }
    Ok(())
}

/// The start runs on the queue's current slot as it stands once the record lands. Requiring the
/// slot the transition chose left the player under `Loading` with nothing on the deck after any
/// of these, Play and Pause doing nothing until Next, Previous or Stop.
#[test]
fn an_edit_during_the_fetch_moves_what_the_deferred_start_plays() -> Result<(), AppError> {
    let rows: [(&str, QueueEdit, &str); 4] = [
        (
            "a remove above the current row",
            |q| {
                q.remove_at(0);
            },
            "/music/2.mp3",
        ),
        ("a shuffle", |q| q.shuffle_play_order_in_place(&mut rand::rng(), true), "/music/2.mp3"),
        (
            "a prune above the current row",
            |q| {
                q.prune_missing(&HashSet::from([1]));
            },
            "/music/2.mp3",
        ),
        (
            "a prune of the track awaited",
            |q| {
                q.prune_missing(&HashSet::from([2]));
            },
            "/music/3.mp3",
        ),
    ];
    for (what, edit, expected) in rows {
        let (mut state, owed) = awaiting_track_two(4)?;
        edit(&mut state.queue);
        land_records(&mut state, &[2, 3, 4]);

        let actions = state.resume_awaited(owed);

        assert_eq!(started_path(&actions), Some(expected), "{what}");
    }
    Ok(())
}

#[test]
fn a_queue_cleared_during_the_fetch_stops_rather_than_starting_anything() -> Result<(), AppError> {
    let (mut state, owed) = awaiting_track_two(3)?;
    state.queue.clear();

    let actions = state.resume_awaited(owed);

    assert_eq!(
        (actions, state.status),
        (vec![PlayerAction::Stop { fade_ms: 0 }], PlaybackStatus::Stopped)
    );
    Ok(())
}

/// The start never defers a second time, so a record that never lands can't have the window task
/// owe, fetch and owe again in a loop. The queue publish that moved the slot is what has it fetch.
#[test]
fn a_deferred_start_whose_record_never_landed_owes_nothing_again() -> Result<(), AppError> {
    let (mut state, owed) = awaiting_track_two(3)?;

    let actions = state.resume_awaited(owed);

    assert_eq!(actions, Vec::new());
    Ok(())
}

/// A fade can't wait on a fetch, so a next the window lacks is no next: the decision drops, and
/// the track drains to an end of stream that can defer.
#[test]
fn a_crossfade_into_a_track_the_window_lacks_is_dropped_without_advancing() {
    let mut state = holding(playing_first_of(2), &[1]);
    state.position_ms = 178_200;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    assert_eq!((actions, state.queue.current_index), (Vec::new(), Some(0)));
}

// --- A gapless handover ---

/// The deck already plays the staged track, so a start here would cut the gapless join it was
/// staged for.
#[test]
fn a_handover_onto_the_queues_next_starts_nothing() {
    let mut state = playing_first_of(2);

    let actions = state.build_gapless_advance_actions(Some(make_summary(2, "Track 2", 200_000)));

    assert_eq!(actions, vec![PlayerAction::UpdatePlayCount(1)]);
}

/// Now Playing describes what the deck plays, which is the staged record from its first sample.
#[test]
fn a_handover_onto_the_queues_next_seats_the_staged_record() {
    let mut state = playing_first_of(2);

    state.build_gapless_advance_actions(Some(make_summary(2, "Track 2", 200_000)));

    assert_eq!(
        (
            state.current_track().map(|t| t.id),
            state.duration_ms,
            state.position_ms,
            state.queue.current_index
        ),
        (Some(2), 200_000, 0, Some(1)),
    );
}

/// An edit inside the last `PRELOAD_LEAD_MS` moved the queue's next after the stage. Seating the
/// staged track would skip a Play Next or play a track just removed, Now Playing naming one and the
/// deck playing another.
#[test]
fn at_a_handover_the_queue_wins_over_a_stale_stage() {
    let mut state = playing_first_of(3);

    let actions = state.build_gapless_advance_actions(Some(make_summary(3, "Track 3", 180_000)));

    assert_eq!(started_path(&actions), Some("/music/2.mp3"));
}

/// A stage whose track was removed, leaving nothing after the current one, ends at the end of the
/// queue as a drained end of stream does.
#[test]
fn a_handover_past_the_end_of_the_queue_stops() {
    let mut state = playing_first_of(1);

    let actions = state.build_gapless_advance_actions(Some(make_summary(2, "Track 2", 180_000)));

    assert_eq!(
        (actions, state.status),
        (
            vec![PlayerAction::UpdatePlayCount(1), PlayerAction::Stop { fade_ms: 0 }],
            PlaybackStatus::Stopped
        )
    );
}

// --- What the window task reads and seats ---

/// `state` behind a handle, as the window task reaches it.
fn handle_on(state: PlayerState) -> PlayerStateHandle {
    let handle = PlayerStateHandle::default();
    *lock_state(&handle) = state;
    handle
}

/// A queue move fetches only what the window lacks, where a library tick may have rewritten any
/// record it holds and reads them all again.
#[test]
fn only_a_library_tick_asks_again_for_the_records_the_window_holds() {
    let rows = [(WindowFill::Missing, vec![3, 4]), (WindowFill::All, vec![1, 2, 3, 4])];
    for (fill, expected) in rows {
        let handle = handle_on(holding(queued(5), &[1, 2]));

        assert_eq!(window_wanted(&handle, fill), expected, "{fill:?}");
    }
}

/// A queue cleared under a playing track leaves it on the deck with no slot, and a retag of it
/// still has to reach Now Playing.
#[test]
fn a_library_tick_asks_for_the_playing_track_a_cleared_queue_no_longer_names() {
    let mut state = playing_first_of(3);
    state.queue.clear();
    let handle = handle_on(state);

    assert_eq!(window_wanted(&handle, WindowFill::All), vec![1]);
}

/// A handle playing track 1 off a one-track queue, its sinks, and a receiver on its view model.
fn emitting_track_one() -> (
    PlayerStateHandle,
    crate::player::engine::event_sink::PlayerSinks,
    tokio::sync::watch::Receiver<Option<PlayerViewModelLight>>,
) {
    let (view_model, published) = tokio::sync::watch::channel(None);
    let (queue, _) = tokio::sync::watch::channel(None);
    let (awaited_start, _) = tokio::sync::watch::channel(None);
    let sinks = crate::player::engine::event_sink::PlayerSinks {
        view_model,
        queue,
        media_controls: None,
        awaited_start,
    };
    (handle_on(playing_first_of(1)), sinks, published)
}

/// Track 1's record titled `title`, as the database holds it once the save has written
/// `position_ms`.
fn saved_at(position_ms: i64, title: &str) -> TrackSummary {
    TrackSummary { last_position: position_ms, ..(*make_summary(1, title, 180_000)).clone() }
}

/// The periodic save rewrites `last_position` as a track plays, so every library tick hands the
/// playing record back with that alone moved. Taken for an edit, it republished the view model on
/// every tick once a track had played a while.
#[test]
fn a_record_moved_only_in_its_saved_position_is_not_republished() {
    let (handle, sinks, published) = emitting_track_one();

    seat_window_records(&handle, &sinks, vec![saved_at(60_000, "Track 1")]);

    assert_eq!(published.has_changed().ok(), Some(false));
}

/// The retag that happens: a track that has played a while comes back with the new tags and the
/// position the save wrote, and setting the position aside must not set the edit aside with it.
#[test]
fn a_retag_of_a_track_that_has_played_a_while_reaches_now_playing() {
    let (handle, sinks, _published) = emitting_track_one();

    seat_window_records(&handle, &sinks, vec![saved_at(60_000, "Retagged")]);

    assert_eq!(
        lock_state(&handle).current_track().map(|t| t.title.clone()),
        Some("Retagged".to_owned())
    );
}
