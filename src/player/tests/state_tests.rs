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
        if resume_pos > 0 {
            Some(resume_pos)
        } else {
            None
        },
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
        if resume_pos > 0 {
            Some(resume_pos)
        } else {
            None
        },
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
    let mut state = PlayerState {
        gapless_enabled: true,
        ..Default::default()
    };

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
    let mut state = PlayerState {
        status: PlaybackStatus::Stopped,
        ..Default::default()
    };

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
    let mut state = PlayerState {
        status: PlaybackStatus::Playing,
        ..Default::default()
    };

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
    let mut state = PlayerState {
        volume: 80,
        ..Default::default()
    };

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
    let mut state = PlayerState {
        playback_speed: 0.1f64.clamp(0.25, 2.0),
        ..Default::default()
    };
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
    let should_skip = state.duration_ms > 0 && state.position_ms < state.duration_ms / 2;
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
    let mut state = PlayerState {
        playback_speed: 1.5,
        ..Default::default()
    };

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
    CrossfadeDecision {
        fade_ms,
        track_id: Some(track_id),
        position_ms,
    }
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

// ── sync_track_summaries ──

#[test]
fn sync_track_summaries_patches_current_queue_and_republishes() {
    use std::collections::HashMap;

    use tokio::sync::watch;

    use crate::player::event_sink::PlayerSinks;

    let handle = PlayerStateHandle::default();
    let (vm_tx, _vm_rx) = watch::channel(None);
    let (q_tx, mut q_rx) = watch::channel(None);
    let sinks = PlayerSinks {
        view_model: vm_tx,
        queue: q_tx,
        media_controls: None,
    };

    // Seed a current track + a coherent two-entry queue.
    with_state_emit(&handle, &sinks, |s| {
        let t1 = make_summary(1, "Old One", 1000);
        let t2 = make_summary(2, "Old Two", 2000);
        s.current_track = Some(Arc::clone(&t1));
        s.queue.tracks = vec![t1, t2];
        s.queue.play_order = vec![0, 1];
        s.queue.current_index = Some(0);
        Vec::<PlayerAction>::new()
    });
    drop(q_rx.borrow_and_update());
    let version_before = lock_state(&handle).queue.version;

    let mut fresh = HashMap::new();
    fresh.insert(1, (*make_summary(1, "New One", 1000)).clone());
    fresh.insert(2, (*make_summary(2, "New Two", 2000)).clone());
    sync_track_summaries(&handle, &sinks, &fresh);

    {
        let g = lock_state(&handle);
        assert_eq!(
            g.current_track.as_ref().map(|t| t.title.as_str()),
            Some("New One"),
            "the currently-playing summary must be refreshed"
        );
        assert_eq!(g.queue.tracks[0].title, "New One");
        assert_eq!(g.queue.tracks[1].title, "New Two");
        assert!(
            g.queue.version > version_before,
            "a queue patch must bump the version so the queue VM republishes"
        );
    }
    assert!(matches!(q_rx.has_changed(), Ok(true)), "the queue view-model must be republished");

    // An edit touching nothing queued/playing is a no-op: no version bump, no publish.
    drop(q_rx.borrow_and_update());
    let version_after = lock_state(&handle).queue.version;
    let mut absent = HashMap::new();
    absent.insert(99, (*make_summary(99, "Nope", 1)).clone());
    sync_track_summaries(&handle, &sinks, &absent);
    assert_eq!(lock_state(&handle).queue.version, version_after, "no-op must not bump version");
    assert!(
        matches!(q_rx.has_changed(), Ok(false)),
        "an edit touching nothing queued must not republish"
    );
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
        (PlayerAction::Seek { position_ms: 9_001 }, &["seek", "9001"]),
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

// --- The radio arm ---------------------------------------------------------
//
// A live stream is the one source with no track behind it, so every transport builder above has a
// branch for it. What these pin is that the branch is taken *and* that the queue underneath comes
// through untouched: stopping a station is supposed to hand the library back exactly as it was.

use crate::player::tests::helpers::test_station as station;

/// A player mid-album, so every "the queue is untouched" assertion has something to be about.
fn playing_a_queue() -> PlayerState {
    let mut state = PlayerState::default();
    state.queue.add_tracks(vec![
        make_summary(1, "One", 180_000),
        make_summary(2, "Two", 180_000),
    ]);
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
    assert_eq!(state.radio.as_ref().map(|r| r.name.as_str()), Some("Example FM"));
    assert!(state.current_track.is_none(), "a station is not a track");
    assert_eq!(state.duration_ms, 0);
    assert_eq!(state.position_ms, 0);
    assert_eq!(state.queue.to_persistable(), queue_before, "the queue must survive verbatim");
}

/// D11: rodio implements speed by reporting a multiplied sample rate, which starves a real-time
/// source. Resetting the state alongside the deck is what keeps the transport honest about it.
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
    assert_eq!(
        actions,
        vec![PlayerAction::PlayStream {
            generation,
            volume: 1.0,
        }]
    );
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
    assert_eq!(state.radio.as_ref().map(|r| r.name.as_str()), Some("Other FM"));
}

#[test]
fn a_failed_connect_forgets_the_station() {
    let (mut state, generation) = tuned_in();

    let actions = state.build_station_failed_actions(generation);

    assert_eq!(actions, vec![PlayerAction::Stop { fade_ms: 0 }]);
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(state.radio.is_none());
}

#[test]
fn a_failure_report_from_a_superseded_session_is_ignored() {
    let (mut state, stale) = tuned_in();
    let (_fresh, _actions) = state.build_station_connecting_actions(station("Other FM"));

    assert_eq!(state.build_station_failed_actions(stale), vec![]);
    assert!(state.radio.is_some(), "the newer station must survive the older one's failure");
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
    assert!(state.radio.is_some(), "the station stays on screen with a play button");
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
    assert!(state.radio.is_none());
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
    assert!(state.radio.is_none());
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

#[test]
fn the_view_model_carries_the_station_instead_of_a_track() {
    let (mut state, generation) = tuned_in();
    let _started = state.build_station_connected_actions(generation);
    if let Some(radio) = state.radio.as_mut() {
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
    let rendered = PlayerAction::PlayStream {
        generation: 7,
        volume: 1.0,
    }
    .to_string();

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

    assert!(state.radio.is_none(), "a track and a station cannot both be the source");
    assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
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
            position_ms: 30_000
        }]
    );
    assert_eq!(state.build_pause_actions(250), vec![PlayerAction::Pause { fade_ms: 250 }]);
    assert_eq!(state.status, PlaybackStatus::Paused, "pausing a track is not a stop");
}
