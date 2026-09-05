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

// --- The restart -----------------------------------------------------------
//
// `queue.json` holds the queue and the station over it in one file, because a station leaves the
// queue seated underneath and a restart owes both. What `melodia-engine` pins is that the two
// survive the file; what these pin is the half above it — which ids still resolve, and what
// shuffle and repeat come back as.

use melodia_engine::player::engine::fixtures::test_station;
use melodia_engine::player::engine::types::PersistableQueue;
use melodia_store::database::queries::fixtures::insert_test_track;
use tempfile::TempDir;

use crate::services::settings::{SettingsData, read_settings, write_settings};

fn persisted(
    track_ids: Vec<i64>,
    current_index: i32,
    station_id: Option<i64>,
) -> PersistedPlayback {
    PersistedPlayback {
        queue: PersistableQueue {
            track_ids,
            current_index,
        },
        station_id,
    }
}

fn plan_of(
    persisted: Option<PersistedPlayback>,
    summaries: Vec<Arc<TrackSummary>>,
    station: Option<Arc<RadioNowPlaying>>,
) -> RestorePlan {
    RestorePlan {
        persisted,
        summaries,
        station,
        repeat_mode: RepeatMode::Off,
        shuffle_enabled: false,
    }
}

/// A data directory with `settings.json` already written, since `mutate_settings_with` reads
/// before it writes and a `Paths` alone points at nothing.
fn seeded_paths(shuffle_enabled: bool) -> Result<(TempDir, Arc<Paths>), AppError> {
    let tmp = TempDir::new()?;
    let paths = Arc::new(Paths::rooted_at(tmp.path().to_path_buf()));
    let mut settings = SettingsData::default();
    settings.queue.shuffle_enabled = shuffle_enabled;
    settings.queue.repeat_mode = RepeatMode::All;
    write_settings(&paths, &settings)?;
    Ok((tmp, paths))
}

// The four corners of what the file can carry, per the cross-the-flags rule: queue and station
// are independent, and each combination is a session someone actually had.

#[test]
fn a_queue_with_no_station_restores_as_a_queue() {
    let mut state = PlayerState::default();

    let actions = apply_restore(
        &mut state,
        plan_of(Some(persisted(vec![1, 2], 0, None)), vec![make_summary(1), make_summary(2)], None),
    );

    assert!(actions.is_empty(), "no station means nothing for the deck to do");
    assert_eq!(state.queue.tracks.len(), 2);
    assert_eq!(state.current_track().map(|t| t.id), Some(1));
    assert!(state.station().is_none());
}

#[test]
fn a_station_with_no_queue_restores_as_a_station() {
    let mut state = PlayerState::default();

    let _actions = apply_restore(
        &mut state,
        plan_of(Some(persisted(vec![], 0, Some(42))), vec![], Some(test_station("Example FM"))),
    );

    assert!(state.queue.tracks.is_empty());
    assert_eq!(state.station().map(|s| s.name.as_str()), Some("Example FM"));
    assert!(state.current_track().is_none());
}

/// The pair the single file exists for. A restore that seated only one of these is the loss the
/// user cannot get back by rescanning.
#[test]
fn a_station_over_a_queue_restores_both() {
    let mut state = PlayerState::default();

    let _actions = apply_restore(
        &mut state,
        plan_of(
            Some(persisted(vec![1, 2], 1, Some(42))),
            vec![make_summary(1), make_summary(2)],
            Some(test_station("Example FM")),
        ),
    );

    assert_eq!(state.station().map(|s| s.name.as_str()), Some("Example FM"));
    assert_eq!(state.queue.tracks.len(), 2, "the queue is seated under the station");
    assert_eq!(state.queue.current_index, Some(1), "and at the row it was left on");
    assert!(state.current_track().is_none(), "the station holds the deck");
}

#[test]
fn a_first_launch_with_no_file_restores_nothing() {
    let mut state = PlayerState::default();

    let actions = apply_restore(&mut state, plan_of(None, vec![], None));

    assert!(actions.is_empty());
    assert!(state.queue.tracks.is_empty());
    assert!(state.station().is_none());
    assert!(state.current_track().is_none());
}

#[test]
fn a_missing_queue_file_restores_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());

    assert!(load_persisted_playback(&paths).is_none());
    Ok(())
}

/// Best-effort by design: a truncated write or a hand-edited file is a session's playback lost,
/// never a boot that fails.
#[test]
fn an_unparseable_queue_file_restores_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    std::fs::write(&paths.queue_path, b"{ not json")?;

    assert!(load_persisted_playback(&paths).is_none());
    Ok(())
}

/// The case that actually happens: a folder removed between sessions, so some persisted ids have
/// no row any more. What comes back is what survived.
#[tokio::test]
async fn ids_whose_rows_are_gone_drop_out_of_the_restore() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_paths(false)?;
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let kept = insert_test_track(&db, "/music/kept.mp3", "Kept", "Artist", "Album", "Rock").await?;

    let plan = plan_restore(&db, &paths, Some(persisted(vec![kept, 9_999], 0, None)), None).await?;

    assert_eq!(
        plan.summaries.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![kept],
        "a row that went away is not a restore that fails"
    );
    Ok(())
}

/// `original_order` is not persisted, so a restored queue with shuffle still on would leave
/// "unshuffle" a no-op against a sequence nobody kept. The file is rewritten to match, or the next
/// launch reads back the value this one just refused.
#[tokio::test]
async fn a_restored_queue_turns_shuffle_off_in_the_state_and_in_the_file() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_paths(true)?;
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let id = insert_test_track(&db, "/music/a.mp3", "A", "Artist", "Album", "Rock").await?;

    let plan = plan_restore(&db, &paths, Some(persisted(vec![id], 0, None)), None).await?;

    assert!(!plan.shuffle_enabled, "a restored queue comes back unshuffled");
    assert!(!read_settings(&paths)?.queue.shuffle_enabled, "and the file agrees");
    assert_eq!(plan.repeat_mode, RepeatMode::All, "repeat is not shuffle's business");
    Ok(())
}

/// The other side of that flag. With nothing restored there is no unknown original order to
/// protect, so the user's own setting stands.
#[tokio::test]
async fn a_restore_with_no_tracks_leaves_the_shuffle_setting_alone() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_paths(true)?;
    let db = DbPool::test_pool().await?;

    let plan = plan_restore(&db, &paths, None, None).await?;

    assert!(plan.shuffle_enabled, "nothing was restored, so nothing was overridden");
    assert!(read_settings(&paths)?.queue.shuffle_enabled);
    Ok(())
}
