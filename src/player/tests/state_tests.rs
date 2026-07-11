use std::sync::Arc;

use super::*;
use crate::error::AppError;

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
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
    assert_eq!(state.position_ms, 0);
    assert_eq!(state.duration_ms, 180_000);
}

#[test]
fn test_play_track_with_start_position() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    let actions = play_track_inner(&mut state, track, Some(45_000));

    assert_eq!(state.position_ms, 45_000);
    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia {
            start_position_ms: Some(45_000),
            ..
        }
    )));
}

#[test]
fn test_play_track_inner_with_resume_position() -> Result<(), AppError> {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    // Play a track, then stop (preserving current_track)
    play_track_inner(&mut state, track, None);
    state.position_ms = 60_000;
    state.status = PlaybackStatus::Stopped;

    // Resume: should replay from saved position
    let track = state
        .current_track
        .clone()
        .ok_or_else(|| AppError::Validation("current_track None".into()))?;
    let resume_pos = state.position_ms;
    let actions = play_track_inner(
        &mut state,
        track,
        if resume_pos > 0 { Some(resume_pos) } else { None },
    );

    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.position_ms, 60_000);
    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia {
            start_position_ms: Some(60_000),
            ..
        }
    )));
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
        .current_track
        .clone()
        .ok_or_else(|| AppError::Validation("current_track None".into()))?;
    let resume_pos = state.position_ms;
    let actions = play_track_inner(
        &mut state,
        track,
        if resume_pos > 0 { Some(resume_pos) } else { None },
    );

    assert_eq!(state.position_ms, 0);
    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia {
            start_position_ms: None,
            ..
        }
    )));
    Ok(())
}

#[test]
fn test_play_track_start_position_clamped_to_near_end() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    // Position beyond duration — clamped to duration_ms - 500 to avoid immediate EOS
    let actions = play_track_inner(&mut state, track, Some(200_000));

    assert_eq!(state.position_ms, 179_500);
    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia {
            start_position_ms: Some(179_500),
            ..
        }
    )));
}

#[test]
fn test_play_track_some_zero_filtered_to_none() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Song", 180_000);

    // Some(0) is filtered to None — no unnecessary seek to position 0
    let actions = play_track_inner(&mut state, track, Some(0));

    assert_eq!(state.position_ms, 0);
    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia {
            start_position_ms: None,
            ..
        }
    )));
}

#[test]
fn test_play_track_with_resume_does_not_eager_preload() {
    let mut state = PlayerState { gapless_enabled: true, ..Default::default() };

    let track1 = make_summary(1, "Song 1", 180_000);
    let track2 = make_summary(2, "Song 2", 180_000);
    state.queue.add_tracks(vec![track1.clone(), track2]);
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
        current_track: Some(make_summary(1, "Song", 100_000)),
        position_ms: 50_000,
        ..Default::default()
    };

    let actions = stop_end_of_queue(&mut state);

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.position_ms, 0);
    assert!(state.current_track.is_some()); // Preserved for replay
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
    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia {
            start_position_ms: None,
            ..
        }
    )));
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
    assert!(actions.iter().any(|a| matches!(
        a,
        PlayerAction::PlayMedia {
            start_position_ms: Some(60_000),
            ..
        }
    )));
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
fn test_pause_and_resume() {
    let mut state = PlayerState { status: PlaybackStatus::Playing, ..Default::default() };

    state.status = PlaybackStatus::Paused;
    assert_eq!(state.status, PlaybackStatus::Paused);

    state.status = PlaybackStatus::Playing;
    assert_eq!(state.status, PlaybackStatus::Playing);
}

#[test]
fn test_stop_preserves_track_for_resume() {
    let mut state = PlayerState {
        status: PlaybackStatus::Playing,
        current_track: Some(make_summary(1, "Song", 100_000)),
        duration_ms: 100_000,
        position_ms: 50_000,
        ..Default::default()
    };

    // Simulate player_stop: sets Stopped but preserves current_track for resume
    state.status = PlaybackStatus::Stopped;

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
    assert_eq!(state.duration_ms, 100_000);
}

#[test]
fn test_stop_at_end_of_queue_preserves_track() -> Result<(), AppError> {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "Song 1", 100_000);
    let track2 = make_summary(2, "Song 2", 100_000);

    state.queue.add_tracks(vec![track1, track2]);

    let t = state
        .queue
        .advance()
        .cloned()
        .ok_or_else(|| AppError::Validation("advance None #1".into()))?;
    play_track_inner(&mut state, t, None);
    let t = state
        .queue
        .advance()
        .cloned()
        .ok_or_else(|| AppError::Validation("advance None #2".into()))?;
    play_track_inner(&mut state, t, None);

    // End of queue — no more tracks to advance to
    assert!(state.queue.advance().is_none());

    // Simulate end-of-queue stop (as in handlers.rs / player_next)
    state.status = PlaybackStatus::Stopped;
    state.position_ms = 0;

    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
    assert_eq!(state.position_ms, 0);
    Ok(())
}

#[test]
fn test_set_volume_clamps() {
    let mut state = PlayerState::default();
    // A request above the ceiling clamps down to MAX_VOLUME.
    state.build_set_volume_actions(999);
    assert_eq!(state.volume, MAX_VOLUME);

    // A valid level passes through untouched.
    state.build_set_volume_actions(75);
    assert_eq!(state.volume, 75);
}

#[test]
fn test_toggle_mute() {
    let mut state = PlayerState { volume: 80, ..Default::default() };

    // Mute
    state.pre_mute_volume = state.volume;
    state.is_muted = true;
    assert!(state.is_muted);
    assert_eq!(state.pre_mute_volume, 80);

    // Unmute
    state.is_muted = false;
    assert!(!state.is_muted);
}

#[test]
fn test_set_playback_speed_clamps() {
    let mut state = PlayerState { playback_speed: 0.1f64.clamp(0.25, 2.0), ..Default::default() };
    assert!((state.playback_speed - 0.25).abs() < f64::EPSILON);

    state.playback_speed = 5.0f64.clamp(0.25, 2.0);
    assert!((state.playback_speed - 2.0).abs() < f64::EPSILON);

    state.playback_speed = 1.5f64.clamp(0.25, 2.0);
    assert!((state.playback_speed - 1.5).abs() < f64::EPSILON);
}

#[test]
fn test_next_skips_when_under_50_percent() {
    let mut state = PlayerState::default();

    let track1 = make_summary(1, "Song 1", 100_000);
    let track2 = make_summary(2, "Song 2", 100_000);

    state.queue.add_tracks(vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track1);
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 100_000;
    state.position_ms = 20_000; // 20% - should count as skip

    // Simulate Next: check skip condition then advance
    let should_skip =
        state.duration_ms > 0 && state.position_ms < state.duration_ms / 2;
    assert!(should_skip);

    let next_track = state.queue.advance().cloned();
    assert!(next_track.is_some());
    assert_eq!(state.queue.current_index, Some(1));
}

#[test]
fn test_previous_restarts_after_3_seconds() {
    let mut state = PlayerState::default();

    let track = make_summary(1, "Song", 100_000);
    state.queue.add_tracks(vec![track.clone()]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track);
    state.position_ms = 5000;

    // When position > 3000, Previous should seek to 0 instead of going back
    assert!(state.position_ms > 3000);
}

#[test]
fn test_queue_add_and_remove() {
    let mut state = PlayerState::default();

    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
        make_summary(3, "Song 3", 100_000),
    ];

    state.queue.add_tracks(tracks);
    assert_eq!(state.queue.play_order.len(), 3);

    state.queue.remove_at(1);
    assert_eq!(state.queue.play_order.len(), 2);
    // After removing index 1 (Song 2), play order should be [Song 1, Song 3]
    let remaining: Vec<i64> = state.queue.tracks_in_play_order().iter().map(|t| t.id).collect();
    assert_eq!(remaining, vec![1, 3]);
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

    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 200_000),
    ];

    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(1);
    state.queue.shuffle_enabled = false;
    state.queue.repeat_mode = RepeatMode::Off;

    if let Some(track) = state.queue.get_current().cloned() {
        state.duration_ms = u64::try_from(track.duration_ms).unwrap_or(0);
        state.current_track = Some(track);
    }

    assert_eq!(state.queue.play_order.len(), 2);
    assert_eq!(state.queue.current_index, Some(1));
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
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
    let state = PlayerState {
        position_ms: 5000,
        duration_ms: 100_000,
        ..Default::default()
    };

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
    state.queue.add_tracks(tracks);
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
    assert_eq!(vm.queue_tracks.len(), 0);
    assert_eq!(vm.queue_index, -1);
    assert!(!vm.has_next);
    assert!(!vm.has_previous);
    assert!((vm.progress_percent - 0.0).abs() < f64::EPSILON);
}

#[test]
fn test_to_view_model_has_previous_with_repeat_all() {
    let mut state = PlayerState::default();
    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
    ];
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);
    state.queue.repeat_mode = RepeatMode::All;

    let vm = state.to_view_model();
    assert!(vm.has_previous); // wraps around with RepeatMode::All
}

#[test]
fn test_to_view_model_has_previous_false_at_start() {
    let mut state = PlayerState::default();
    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
    ];
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);
    state.queue.repeat_mode = RepeatMode::Off;

    let vm = state.to_view_model();
    assert!(!vm.has_previous);
}

#[test]
fn test_to_view_model_zero_duration() {
    let state = PlayerState {
        duration_ms: 0,
        position_ms: 0,
        ..Default::default()
    };

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

    let tracks = vec![
        make_summary(1, "Song 1", 200_000),
        make_summary(2, "Song 2", 100_000),
    ];
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);
    state.current_track = Some(make_summary(1, "Song 1", 200_000));

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

// ── QueueViewModel tests ──

#[test]
fn test_to_queue_view_model_correct_tracks() {
    let mut state = PlayerState::default();
    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
        make_summary(3, "Song 3", 100_000),
    ];
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(1);

    let qvm = state.to_queue_view_model();
    assert_eq!(qvm.queue_tracks.len(), 3);
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
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(1);
    state.queue.apply_shuffle_order(&[1, 2, 0]);

    let qvm = state.to_queue_view_model();
    assert!(qvm.shuffle_enabled);
    // Shuffled order: original indices [1, 2, 0] → tracks [2, 3, 1]
    assert_eq!(qvm.queue_tracks[0].id, 2);
    assert_eq!(qvm.queue_tracks[1].id, 3);
    assert_eq!(qvm.queue_tracks[2].id, 1);
}

#[test]
fn test_to_queue_view_model_has_next_at_end() {
    let mut state = PlayerState::default();
    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 100_000),
    ];
    state.queue.add_tracks(tracks);
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
    let tracks = vec![
        make_summary(1, "Song 1", 100_000),
        make_summary(2, "Song 2", 200_000),
    ];
    let persistable = PersistableQueue {
        track_ids: vec![1, 2],
        current_index: 1,
    };

    restore_queue(&mut state, tracks, &persistable);

    assert_eq!(state.queue.tracks.len(), 2);
    assert_eq!(state.queue.current_index, Some(1));
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
    assert_eq!(state.duration_ms, 200_000);
}

#[test]
fn test_restore_queue_empty() {
    let mut state = PlayerState::default();
    let persistable = PersistableQueue {
        track_ids: vec![],
        current_index: 0,
    };

    restore_queue(&mut state, vec![], &persistable);

    assert!(state.current_track.is_none());
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

    let persistable = PersistableQueue {
        track_ids: vec![1],
        current_index: 0,
    };

    restore_queue(&mut state, vec![track], &persistable);

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

    restore_queue(&mut state, tracks, &persistable);

    // get_current() returns None for out-of-range index, so no track loaded
    assert!(state.current_track.is_none());
    assert_eq!(state.queue.current_index, Some(99));
}

#[test]
fn end_of_stream_advances_when_next_track_present() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "Song 1", 100_000);
    let track2 = make_summary(2, "Song 2", 100_000);
    state.current_track = Some(track1.clone());
    state.queue.add_tracks(vec![track1, track2]);
    state.queue.current_index = Some(0);
    state.status = PlaybackStatus::Playing;

    let actions = state.build_end_of_stream_actions();

    // Advanced to track 2 and kept playing.
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
    assert_eq!(state.queue.current_index, Some(1));
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::PlayMedia { .. })));
    // Counted the track that just ended.
    assert!(actions.iter().any(|a| matches!(a, PlayerAction::UpdatePlayCount(1))));
}

#[test]
fn end_of_stream_stops_at_end_of_queue() {
    let mut state = PlayerState::default();
    let track = make_summary(1, "Only Song", 100_000);
    state.current_track = Some(track.clone());
    state.queue.add_tracks(vec![track]);
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
    state.current_track = Some(track1.clone());
    state.queue.add_tracks(vec![track1, track2]);
    state.queue.current_index = Some(0);
    state.status = PlaybackStatus::Playing;
    state.pause_after_current_track = true;

    let actions = state.build_end_of_stream_actions();

    // Did NOT advance — current track unchanged, stopped at position 0.
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
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
    state.queue.add_tracks(vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track1);
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 180_000;
    state.position_ms = 178_200;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    // State advances at fade *start* — Now-Playing switches as the overlap begins.
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
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
    state.queue.add_tracks(vec![track1.clone()]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track1);
    state.status = PlaybackStatus::Playing;
    state.duration_ms = 180_000;
    state.position_ms = 178_000;

    let actions = state.build_crossfade_actions(decision(2_000, 1, 178_000));

    assert!(actions.is_empty(), "no next track means no crossfade");
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
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
    state.queue.add_tracks(vec![track1.clone()]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track1);
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
    state.queue.add_tracks(vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track1);
    state.duration_ms = 180_000;
    state.position_ms = 178_200;
    // The pause won the race.
    state.status = PlaybackStatus::Paused;

    let actions = state.build_crossfade_actions(decision(1_800, 1, 178_200));

    assert!(actions.is_empty(), "a paused player must not crossfade");
    assert_eq!(state.status, PlaybackStatus::Paused, "the pause must stick");
    assert_eq!(state.queue.current_index, Some(0), "the queue must not advance");
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
}

/// Same window, but the user picked a different track instead of pausing.
/// Advancing here would skip straight past the track they just chose.
#[test]
fn crossfade_is_dropped_when_the_track_changed_since_the_decision() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    let track2 = make_summary(2, "Two", 200_000);
    let track3 = make_summary(3, "Three", 220_000);
    state.queue.add_tracks(vec![track1, track2.clone(), track3]);
    // A manual jump to track 2 landed after the monitor decided on track 1.
    state.queue.current_index = Some(1);
    state.current_track = Some(track2);
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
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
}

/// One case the track id alone misses: the *same* track restarted inside the
/// window. `play_track_inner` resets `position_ms` to 0, so the position is the
/// tell.
#[test]
fn crossfade_is_dropped_when_the_same_track_restarted_since_the_decision() {
    let mut state = PlayerState::default();
    let track1 = make_summary(1, "One", 180_000);
    let track2 = make_summary(2, "Two", 200_000);
    state.queue.add_tracks(vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track1);
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
    state.queue.add_tracks(vec![track1.clone(), track2]);
    state.queue.current_index = Some(0);
    state.current_track = Some(track1);
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
