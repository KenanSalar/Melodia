use std::sync::Arc;

use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_engine::player::engine::state::PlayerState;

use super::*;

fn make_summary(id: i64) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: format!("/music/{id}.mp3"),
        file_name: format!("{id}.mp3"),
        title: format!("Track {id}"),
        artist: None,
        album: None,
        duration_ms: 180_000,
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
fn shuffle_inline_empty_queue_noop() {
    let mut state = PlayerState::default();
    shuffle_inline(&mut state);
    assert!(!state.queue.shuffle_enabled);
}

#[test]
fn shuffle_inline_single_track() {
    let mut state = PlayerState::default();
    state.queue.add_tracks(vec![make_summary(1)]);
    state.queue.current_index = Some(0);

    shuffle_inline(&mut state);

    assert!(state.queue.shuffle_enabled);
    assert_eq!(state.queue.get_current().map(|t| t.id), Some(1));
}

#[test]
fn shuffle_inline_keeps_current_at_front() {
    let mut state = PlayerState::default();
    let tracks: Vec<_> = (1..=10).map(make_summary).collect();
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(5); // Track 6 is current

    shuffle_inline(&mut state);

    assert_eq!(state.queue.get_current().map(|t| t.id), Some(6));
}

#[test]
fn shuffle_inline_enables_shuffle_flag() {
    let mut state = PlayerState::default();
    state.queue.add_tracks(vec![make_summary(1), make_summary(2), make_summary(3)]);
    state.queue.current_index = Some(0);

    assert!(!state.queue.shuffle_enabled);
    shuffle_inline(&mut state);
    assert!(state.queue.shuffle_enabled);
}

#[test]
fn shuffle_inline_all_indices_present() -> Result<(), AppError> {
    let mut state = PlayerState::default();
    let count: i64 = 20;
    let tracks: Vec<_> = (1..=count).map(make_summary).collect();
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);

    shuffle_inline(&mut state);

    let play_order = state.queue.tracks_in_play_order();
    let expected_len =
        usize::try_from(count).map_err(|_| AppError::Validation("count negative".into()))?;
    assert_eq!(play_order.len(), expected_len);

    let mut ids: Vec<i64> = play_order.iter().map(|t| t.id).collect();
    ids.sort_unstable();
    let expected: Vec<i64> = (1..=count).collect();
    assert_eq!(ids, expected);
    Ok(())
}

#[test]
fn shuffle_unshuffle_roundtrip() {
    let mut state = PlayerState::default();
    let tracks: Vec<_> = (1..=5).map(make_summary).collect();
    let original_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);

    shuffle_inline(&mut state);
    assert!(state.queue.shuffle_enabled);

    state.queue.unshuffle();
    assert!(!state.queue.shuffle_enabled);

    let restored: Vec<i64> = state.queue.tracks_in_play_order().iter().map(|t| t.id).collect();
    assert_eq!(restored, original_ids);
}

#[test]
fn queue_set_shuffle_noop_when_already_enabled() {
    let mut state = PlayerState::default();
    let tracks: Vec<_> = (1..=5).map(make_summary).collect();
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);

    shuffle_inline(&mut state);
    assert!(state.queue.shuffle_enabled);

    let already_enabled = state.queue.shuffle_enabled;
    assert!(already_enabled);
}

#[test]
fn queue_toggle_shuffle_enables_then_disables() {
    let mut state = PlayerState::default();
    let tracks: Vec<_> = (1..=5).map(make_summary).collect();
    state.queue.add_tracks(tracks);
    state.queue.current_index = Some(0);

    if state.queue.shuffle_enabled {
        state.queue.unshuffle();
    } else {
        shuffle_inline(&mut state);
    }
    assert!(state.queue.shuffle_enabled);

    if state.queue.shuffle_enabled {
        state.queue.unshuffle();
    } else {
        shuffle_inline(&mut state);
    }
    assert!(!state.queue.shuffle_enabled);
}

#[test]
fn queue_cycle_repeat_cycles_correctly() {
    let mut state = PlayerState::default();
    assert_eq!(state.queue.repeat_mode, RepeatMode::Off);

    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::All);

    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::One);

    state.queue.cycle_repeat_mode();
    assert_eq!(state.queue.repeat_mode, RepeatMode::Off);
}
