use std::sync::Arc;

use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_engine::player::engine::state::{PlayerState, lock_state};

use super::*;
use crate::state::fixtures::test_sinks;

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

// --- The three transport doors ---------------------------------------------
//
// Driven through the workers under `queue_{set_shuffle,toggle_shuffle,cycle_repeat}` rather than
// by rebuilding their bodies over a bare `PlayerState`, which is what the tests these replaced
// did: none of the three ever called the door it was named after, so an instrumented run reported
// all three doors never executed while all three tests passed. What the doors add over the queue
// methods `melodia-engine` already pins is the branch they pick and the value they hand
// `persist_{shuffle,repeat}`, so that is what these assert.

fn seated_queue(count: i64) -> (PlayerStateHandle, PlayerSinks) {
    let player_state = PlayerStateHandle::default();
    let sinks = test_sinks();
    with_state_emit(&player_state, &sinks, |s| {
        s.queue.add_tracks((1..=count).map(make_summary).collect());
        s.queue.current_index = Some(0);
    });
    (player_state, sinks)
}

fn queue_version(player_state: &PlayerStateHandle) -> u64 {
    lock_state(player_state).queue.version
}

/// `queue.version` is the witness rather than the play order: re-shuffling five tracks lands on
/// the same permutation often enough that an order comparison would pass at random, and the
/// version is what `with_state_emit` gates the queue re-emit on anyway.
#[test]
fn a_shuffle_asked_for_twice_reorders_once() {
    let (player_state, sinks) = seated_queue(5);
    set_shuffle(&player_state, &sinks, true);
    let after_first = queue_version(&player_state);

    set_shuffle(&player_state, &sinks, true);

    assert_eq!(
        queue_version(&player_state),
        after_first,
        "the Shuffle pill is pressed twice by a caller that already asked for it, and a second \
         reorder throws away the position the listener was at"
    );
}

/// The one input where the request and the outcome disagree, and the reason the door answers with
/// the state: `persist_shuffle` takes this value, so a version returning `enabled` writes a
/// shuffle into `settings.json` that the next launch restores over a queue nothing reordered.
#[test]
fn shuffling_an_empty_queue_answers_that_nothing_was_shuffled() {
    let (player_state, sinks) = seated_queue(0);

    let settled = set_shuffle(&player_state, &sinks, true);

    assert!(!settled, "an empty queue has no order to shuffle, so the request cannot be granted");
}

/// Pins the door's `false` arm. The shuffled order is arranged by hand rather than by asking for
/// one, so nothing here depends on the RNG having moved anything: a version clearing the flag
/// without calling `unshuffle` leaves the queue reversed, and gets caught every run instead of
/// once in a hundred and twenty.
#[test]
fn turning_shuffle_off_puts_the_queue_back_in_the_order_it_was_added() {
    let (player_state, sinks) = seated_queue(5);
    with_state_emit(&player_state, &sinks, |s| {
        s.queue.play_order.reverse();
        s.queue.shuffle_enabled = true;
    });

    set_shuffle(&player_state, &sinks, false);

    let state = lock_state(&player_state);
    assert!(!state.queue.shuffle_enabled, "the request was `false`");
    let restored: Vec<i64> = state.queue.tracks_in_play_order().iter().map(|t| t.id).collect();
    assert_eq!(restored, (1..=5).collect::<Vec<i64>>());
}

#[test]
fn the_toggle_flips_whichever_way_shuffle_is_currently_pointing() {
    let (player_state, sinks) = seated_queue(5);

    assert!(toggle_shuffle(&player_state, &sinks), "a queue in order shuffles");
    assert!(!toggle_shuffle(&player_state, &sinks), "a shuffled queue goes back in order");
}

/// The answer is what `persist_repeat` writes, so reporting the mode it left rather than the one
/// it landed on brings the next launch back one press behind. `cycle_repeat_mode`'s own sequence
/// is `melodia-engine`'s claim and is pinned there.
#[test]
fn a_repeat_press_answers_with_the_mode_it_landed_on() {
    let (player_state, sinks) = seated_queue(3);

    let announced = cycle_repeat(&player_state, &sinks);

    assert_eq!(announced, RepeatMode::All, "one press off `Off` lands on `All`");
    assert_eq!(announced, lock_state(&player_state).queue.repeat_mode);
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

use crate::services::settings::read_settings;
use crate::state::fixtures::seeded_root_with;

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
    let (tmp, paths) = seeded_root_with(|s| {
        s.queue.shuffle_enabled = shuffle_enabled;
        s.queue.repeat_mode = RepeatMode::All;
    })?;
    Ok((tmp, Arc::new(paths)))
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

// --- what a drop and a "Play Next" owe their input order ---

/// A `TrackSummary` with a title of its own, since [`make_summary`] ties one to the id and the
/// ordering tests need the two to vary apart.
fn titled(id: i64, title: &str) -> Arc<TrackSummary> {
    let mut summary = (*make_summary(id)).clone();
    summary.title = title.to_owned();
    Arc::new(summary)
}

/// Files from outside arrive in visual selection order on KDE and GNOME, not alphabetical, and
/// `natord` is what puts "Track 2" ahead of "Track 10".
#[test]
fn a_dropped_batch_is_ordered_the_way_a_list_is() {
    let mut batch = vec![
        titled(1, "Track 10"),
        titled(2, "Track 9"),
        titled(3, "Track 1"),
    ];

    sort_for_queue(&mut batch);

    assert_eq!(
        batch.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        ["Track 1", "Track 9", "Track 10"],
        "a byte compare puts 10 ahead of 9, which is every numbered album out of order"
    );
}

/// The sort is stable, so a batch the tags cannot tell apart keeps the order the file manager
/// handed it over in rather than an arbitrary one.
#[test]
fn equal_titles_keep_the_order_the_drop_handed_over() {
    let mut batch = vec![
        titled(7, "Untitled"),
        titled(3, "Untitled"),
        titled(5, "Untitled"),
    ];

    sort_for_queue(&mut batch);

    assert_eq!(batch.iter().map(|t| t.id).collect::<Vec<_>>(), [7, 3, 5]);
}

/// "Play Next" on a multi-row selection has to land in the order the rows were listed. Every
/// `insert_next` goes to `current_index + 1`, so the batch is walked backwards to come out
/// forwards, and nothing else in the queue moves.
#[tokio::test]
async fn play_next_lands_a_batch_in_input_order_behind_the_current_track() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let mut picked = Vec::new();
    for n in 1..=3 {
        let title = format!("Pick {n}");
        picked.push(
            insert_test_track(&db, &format!("/music/pick{n}.mp3"), &title, "A", "B", "Rock")
                .await?,
        );
    }

    let player_state = PlayerStateHandle::default();
    let sinks = test_sinks();
    with_state_emit(&player_state, &sinks, |s| {
        s.queue.add_tracks(vec![make_summary(100), make_summary(200)]);
        s.queue.current_index = Some(0);
    });

    play_next_many(&db, &player_state, &sinks, &picked).await?;

    let state = lock_state(&player_state);
    let mut expected = vec![100];
    expected.extend(picked.iter().copied());
    expected.push(200);
    assert_eq!(
        state.queue.tracks_in_play_order().iter().map(|t| t.id).collect::<Vec<_>>(),
        expected,
        "the selection lands behind what is playing, in the order it was picked"
    );
    Ok(())
}

/// The context menu can fire on an empty selection, and the guard is what keeps that off the
/// database and off the view-model channel.
#[tokio::test]
async fn play_next_with_nothing_picked_does_not_reach_the_queue() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let player_state = PlayerStateHandle::default();
    let sinks = test_sinks();
    let mut published = sinks.view_model.subscribe();
    published.borrow_and_update();

    play_next_many(&db, &player_state, &sinks, &[]).await?;

    assert!(!published.has_changed().unwrap_or(true), "nothing was picked, so nothing changed");
    assert!(lock_state(&player_state).queue.tracks.is_empty());
    Ok(())
}

// --- Opening a file from outside the app ------------------------------------
//
// The Rust end of the file-association handoff, and the only half of it nothing pinned. `%F` on
// the four `.desktop` sources, `wix/main.wxs`'s `FileAssociations`, the single-instance claim and
// `boot::tasks::serve_file_opens`' backlog are all held somewhere; what they hand to was not.
//
// The queue order survives the round trip because `get_track_summaries_by_ids` re-orders its rows
// to match the ids it was given, so what `open_as_queue` sorts is what ends up on screen.

use melodia_artwork::media::image::artwork::new_cover_cache;
use melodia_core::entities::tags::{FieldEdit, TagEdit};

use crate::state::fixtures::TestPlayback;

/// Stage the silence fixture under `file_name` carrying `title`, and spell its path the way a
/// file manager would. The title has to go in the file: this path imports rather than inserting,
/// so the tag is the only place the sort can read a name from.
fn opened_file(dir: &std::path::Path, file_name: &str, title: &str) -> Result<String, AppError> {
    let dest = dir.join(file_name);
    std::fs::copy(
        std::path::PathBuf::from(melodia_testkit::ASSETS_DIR).join("silence.mp3"),
        &dest,
    )?;
    let edit = TagEdit {
        title: FieldEdit::Set(title.to_owned()),
        ..TagEdit::default()
    };
    melodia_store::media::ingest::tag_writer::apply_to_file(&dest, &edit, None)?;
    Ok(dest.to_string_lossy().into_owned())
}

fn queued_titles(player_state: &PlayerStateHandle) -> Vec<String> {
    lock_state(player_state)
        .queue
        .tracks_in_play_order()
        .iter()
        .map(|track| track.title.clone())
        .collect()
}

/// The order the user sees is the sorted one, not the order the file manager handed over and not
/// the order the ids came back in. `natord` is what makes the three distinguishable: dropped in
/// reverse, "Track 10" sorts after "Track 2" where a plain string compare puts it first.
#[tokio::test]
async fn opened_files_reach_the_queue_in_natural_title_order() -> Result<(), AppError> {
    let fx = TestPlayback::empty().await?;
    let dir = fx.tmp.path();
    let opened = vec![
        opened_file(dir, "c.mp3", "Track 10")?,
        opened_file(dir, "b.mp3", "Track 2")?,
        opened_file(dir, "a.mp3", "Track 1")?,
    ];

    open_as_queue(&fx.ctx, &new_cover_cache(), &opened).await?;

    assert_eq!(queued_titles(&fx.ctx.player_state), ["Track 1", "Track 2", "Track 10"]);
    Ok(())
}

/// The sibling above pins the list; this pins which of it plays. A queue in the right order with
/// `current_index` somewhere else in it is the same bug from the listener's side, and only this
/// notices. (`Some(0)` and `None` are the same call here, `resolve_start_slot` falling back to
/// the head, so that is not what this is about.)
#[tokio::test]
async fn opening_a_batch_starts_at_the_first_track_in_order() -> Result<(), AppError> {
    let fx = TestPlayback::empty().await?;
    let dir = fx.tmp.path();
    let opened = vec![
        opened_file(dir, "b.mp3", "Second")?,
        opened_file(dir, "a.mp3", "First")?,
    ];

    open_as_queue(&fx.ctx, &new_cover_cache(), &opened).await?;

    assert_eq!(
        lock_state(&fx.ctx.player_state).queue.get_current().map(|track| track.title.clone()),
        Some("First".to_owned())
    );
    Ok(())
}

/// Opening replaces; importing appends. The two differ in nothing else, and collapsing them
/// costs the listener the queue they were already playing.
#[tokio::test]
async fn opening_replaces_the_queue_where_importing_appends_to_it() -> Result<(), AppError> {
    let fx = TestPlayback::empty().await?;
    let dir = fx.tmp.path();
    let seated = vec![opened_file(dir, "seated.mp3", "Already Playing")?];
    let arriving = vec![opened_file(dir, "arriving.mp3", "Just Opened")?];

    append_imported(&fx.ctx, &new_cover_cache(), &seated).await?;
    append_imported(&fx.ctx, &new_cover_cache(), &arriving).await?;
    let appended = queued_titles(&fx.ctx.player_state);

    open_as_queue(&fx.ctx, &new_cover_cache(), &arriving).await?;

    assert_eq!(appended, ["Already Playing", "Just Opened"], "the import kept what was there");
    assert_eq!(queued_titles(&fx.ctx.player_state), ["Just Opened"], "the open did not");
    Ok(())
}

/// A batch where nothing could be read is an error rather than an empty queue: the user
/// double-clicked something and is owed an answer, and the caller has no other way to tell that
/// the queue they were listening to is still the right one.
#[tokio::test]
async fn opening_files_that_cannot_be_read_is_an_error_rather_than_an_empty_queue()
-> Result<(), AppError> {
    let fx = TestPlayback::empty().await?;
    let missing = fx.tmp.path().join("never-existed.mp3").to_string_lossy().into_owned();

    let refused = open_as_queue(&fx.ctx, &new_cover_cache(), &[missing]).await;

    assert!(matches!(refused, Err(AppError::Queue(_))), "got {refused:?}");
    assert!(lock_state(&fx.ctx.player_state).queue.tracks.is_empty());
    Ok(())
}
