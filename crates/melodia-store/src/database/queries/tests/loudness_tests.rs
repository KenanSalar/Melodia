//! Loudness measured for files carrying no gain tags: when a stored measurement plays, what a pass
//! is owed, what its writes keep, and what sends an album back to be measured whole again.
//!
//! The gains and peaks are dyadic, so a value written and read back unchanged compares exactly.

use super::{AlbumJob, MeasureTrack, Pending};
use crate::database::DbPool;
use crate::database::queries::fixtures::{
    execute, fetch_one, insert_tagged_track, insert_test_track, make_test_metadata,
};
use crate::database::queries::scan::{RetaggedFile, commit_retag};
use crate::database::queries::{self, loudness};
use melodia_audio::loudness::{AlbumOutcome, Gain, TrackOutcome};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::loudness::GainOrigin;
use melodia_core::entities::tags::TagEdit;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;

const TRACK_GAIN: Gain = Gain { db: -4.5, peak: 0.5 };
const ALBUM_GAIN: Gain = Gain { db: -1.5, peak: 0.75 };

/// The four gains a track plays with, and whether a measurement stood in for its tags.
type Plays = (Option<f32>, Option<f32>, Option<f32>, Option<f32>, GainOrigin);

/// Nothing at all to level by.
const UNLEVELLED: Plays = (None, None, None, None, GainOrigin::Tagged);

/// A library folder at `/music`, folder 1, and an outside one at `/downloads`, folder 2.
async fn library() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    queries::folder::insert_folder(&db, "/downloads", false).await?;
    Ok(db)
}

/// An untagged track at `/music/{title}.flac`, on `album` or on none where it is empty.
async fn track(db: &DbPool, title: &str, album: &str) -> Result<i64, AppError> {
    insert_test_track(db, &format!("/music/{title}.flac"), title, "Artist", album, "Rock").await
}

async fn album_of(db: &DbPool, track: i64) -> Result<i64, AppError> {
    fetch_one(db, &format!("SELECT album_id FROM tracks WHERE id = {track}")).await
}

async fn owed_track(db: &DbPool, id: i64) -> Result<MeasureTrack, AppError> {
    let owed = loudness::track_jobs_after(db, 0, 100).await?;
    owed.into_iter()
        .find(|track| track.id == id)
        .ok_or_else(|| AppError::validation_msg(format!("track {id} is not owed alone")))
}

async fn owed_album(db: &DbPool, album_id: i64) -> Result<AlbumJob, AppError> {
    let owed = loudness::album_jobs_after(db, 0, 100).await?;
    owed.into_iter()
        .find(|album| album.album_id == album_id)
        .ok_or_else(|| AppError::validation_msg(format!("album {album_id} is not owed whole")))
}

/// Records `gain` on `id`, a track owed alone, as a pass would.
async fn measure_alone(db: &DbPool, id: i64, gain: Gain) -> Result<(), AppError> {
    let job = owed_track(db, id).await?;
    loudness::record_track(db, &job, TrackOutcome::Measured(gain)).await?;
    Ok(())
}

/// Records every member of `job` at [`TRACK_GAIN`], and the album at `album`.
async fn record_whole(db: &DbPool, job: &AlbumJob, album: Option<Gain>) -> Result<(), AppError> {
    let tracks = vec![TrackOutcome::Measured(TRACK_GAIN); job.members.len()];
    loudness::record_album(db, job, &AlbumOutcome { tracks, album }).await?;
    Ok(())
}

/// [`record_whole`] for the album `member` is on, at [`ALBUM_GAIN`].
async fn measure_whole(db: &DbPool, member: i64) -> Result<(), AppError> {
    let job = owed_album(db, album_of(db, member).await?).await?;
    record_whole(db, &job, Some(ALBUM_GAIN)).await
}

async fn summary(db: &DbPool, id: i64) -> Result<TrackSummary, AppError> {
    let rows = queries::track::get_track_summaries_by_ids(db, vec![id]).await?;
    rows.into_iter().next().ok_or_else(|| AppError::validation_msg(format!("no summary for {id}")))
}

async fn plays(db: &DbPool, id: i64) -> Result<Plays, AppError> {
    let s = summary(db, id).await?;
    Ok((
        s.replaygain_track_gain,
        s.replaygain_track_peak,
        s.replaygain_album_gain,
        s.replaygain_album_peak,
        s.replaygain_origin,
    ))
}

async fn origin(db: &DbPool, id: i64) -> Result<GainOrigin, AppError> {
    Ok(summary(db, id).await?.replaygain_origin)
}

async fn rows_holding_album_values(db: &DbPool) -> Result<i64, AppError> {
    fetch_one(db, "SELECT COUNT(*) FROM track_loudness WHERE album_id IS NOT NULL").await
}

async fn measurements(db: &DbPool) -> Result<i64, AppError> {
    fetch_one(db, "SELECT COUNT(*) FROM track_loudness").await
}

async fn owed_album_ids(db: &DbPool) -> Result<Vec<i64>, AppError> {
    let owed = loudness::album_jobs_after(db, 0, 100).await?;
    Ok(owed.iter().map(|album| album.album_id).collect())
}

async fn owed_track_ids(db: &DbPool) -> Result<Vec<i64>, AppError> {
    let owed = loudness::track_jobs_after(db, 0, 100).await?;
    Ok(owed.iter().map(|track| track.id).collect())
}

/// An untagged track on [`make_test_metadata`]'s album at `disc` and `number`.
async fn numbered(
    db: &DbPool,
    title: &str,
    disc: i32,
    number: Option<i32>,
) -> Result<i64, AppError> {
    let mut meta = make_test_metadata(title);
    meta.disc_number = Some(disc);
    meta.track_number = number;
    insert_tagged_track(db, &format!("/music/{title}.flac"), &meta).await
}

/// A sheet's second and last track, its span open-ended.
async fn last_sheet_track(db: &DbPool) -> Result<i64, AppError> {
    let id = track(db, "Last", "").await?;
    execute(db, &format!("UPDATE tracks SET cue_track = 2, cue_start = 75 WHERE id = {id}"))
        .await?;
    Ok(id)
}

#[tokio::test]
async fn an_untagged_track_plays_its_measurement() -> Result<(), AppError> {
    let db = library().await?;
    let id = track(&db, "Bare", "").await?;
    measure_alone(&db, id, TRACK_GAIN).await?;

    let plays = plays(&db, id).await?;

    assert_eq!(plays, (Some(-4.5), Some(0.5), None, None, GainOrigin::Measured));
    Ok(())
}

/// A sheet's last track has no end, which an equality would never match.
#[tokio::test]
async fn a_sheet_s_last_track_plays_its_measurement() -> Result<(), AppError> {
    let db = library().await?;
    let id = last_sheet_track(&db).await?;
    measure_alone(&db, id, TRACK_GAIN).await?;

    let origin = origin(&db, id).await?;

    assert_eq!(origin, GainOrigin::Measured);
    Ok(())
}

/// A measurement applies only to the audio it heard: a re-read moving the hash, or a sheet edit
/// moving the span, leaves the track unlevelled until a pass measures it again.
#[tokio::test]
async fn a_measurement_stops_playing_once_the_audio_it_heard_moves() -> Result<(), AppError> {
    let moves = [
        ("the file replaced", "UPDATE tracks SET file_hash = 'replaced'"),
        ("the span's start moved", "UPDATE tracks SET cue_start = 150"),
        ("the span given an end", "UPDATE tracks SET cue_end = 7500"),
    ];
    for (moved, statement) in moves {
        let db = library().await?;
        let id = last_sheet_track(&db).await?;
        measure_alone(&db, id, TRACK_GAIN).await?;
        execute(&db, statement).await?;

        let plays = plays(&db, id).await?;

        assert_eq!(plays, UNLEVELLED, "{moved}");
    }
    Ok(())
}

/// A file carrying only an album gain keeps an empty track pair rather than the measurement's.
#[tokio::test]
async fn either_gain_tag_keeps_a_file_on_its_tags() -> Result<(), AppError> {
    let db = library().await?;
    let id = track(&db, "Bare", "").await?;
    measure_alone(&db, id, TRACK_GAIN).await?;
    execute(&db, &format!("UPDATE tracks SET replaygain_album_gain = -2.0 WHERE id = {id}"))
        .await?;

    let plays = plays(&db, id).await?;

    assert_eq!(plays, (None, None, Some(-2.0), None, GainOrigin::Tagged));
    Ok(())
}

#[tokio::test]
async fn a_peak_tag_never_guards_a_measured_gain() -> Result<(), AppError> {
    let db = library().await?;
    let id = track(&db, "Bare", "").await?;
    execute(&db, &format!("UPDATE tracks SET replaygain_track_peak = 0.25 WHERE id = {id}"))
        .await?;
    measure_alone(&db, id, TRACK_GAIN).await?;

    let s = summary(&db, id).await?;

    assert_eq!((s.replaygain_track_gain, s.replaygain_track_peak), (Some(-4.5), Some(0.5)));
    Ok(())
}

#[tokio::test]
async fn a_whole_album_plays_its_album_gain_on_every_member() -> Result<(), AppError> {
    let db = library().await?;
    let first = track(&db, "A", "Tones").await?;
    let second = track(&db, "B", "Tones").await?;
    measure_whole(&db, first).await?;

    let plays = [plays(&db, first).await?, plays(&db, second).await?];

    let measured = (Some(-4.5), Some(0.5), Some(-1.5), Some(0.75), GainOrigin::Measured);
    assert_eq!(plays, [measured, measured]);
    Ok(())
}

/// An edit moving a track to another album stops it playing the album gain at once, ahead of any
/// pass.
#[tokio::test]
async fn album_values_play_only_on_the_album_they_were_measured_over() -> Result<(), AppError> {
    let db = library().await?;
    let moved = track(&db, "A", "Tones").await?;
    track(&db, "B", "Tones").await?;
    let elsewhere = album_of(&db, track(&db, "C", "Elsewhere").await?).await?;
    measure_whole(&db, moved).await?;
    execute(&db, &format!("UPDATE tracks SET album_id = {elsewhere} WHERE id = {moved}")).await?;

    let plays = plays(&db, moved).await?;

    assert_eq!(plays, (Some(-4.5), Some(0.5), None, None, GainOrigin::Measured));
    Ok(())
}

#[tokio::test]
async fn a_track_that_moved_since_its_job_was_read_records_nothing() -> Result<(), AppError> {
    let moves = [
        ("re-read under a new hash", "UPDATE tracks SET file_hash = 'replaced'"),
        ("tagged", "UPDATE tracks SET replaygain_track_gain = -2.0"),
    ];
    for (moved, statement) in moves {
        let db = library().await?;
        let id = track(&db, "Bare", "").await?;
        let job = owed_track(&db, id).await?;
        execute(&db, statement).await?;

        loudness::record_track(&db, &job, TrackOutcome::Measured(TRACK_GAIN)).await?;

        assert_eq!(measurements(&db).await?, 0, "{moved}");
    }
    Ok(())
}

#[tokio::test]
async fn an_album_with_a_member_that_moved_gets_no_album_values() -> Result<(), AppError> {
    let db = library().await?;
    let first = track(&db, "A", "Tones").await?;
    let moved = track(&db, "B", "Tones").await?;
    let job = owed_album(&db, album_of(&db, first).await?).await?;
    execute(&db, &format!("UPDATE tracks SET file_hash = 'replaced' WHERE id = {moved}")).await?;

    record_whole(&db, &job, Some(ALBUM_GAIN)).await?;

    assert_eq!(rows_holding_album_values(&db).await?, 0);
    Ok(())
}

#[tokio::test]
async fn an_album_with_a_member_out_of_reach_stays_owed() -> Result<(), AppError> {
    let db = library().await?;
    let first = track(&db, "A", "Tones").await?;
    track(&db, "B", "Tones").await?;
    let album = album_of(&db, first).await?;
    let job = owed_album(&db, album).await?;
    let tracks = vec![TrackOutcome::Measured(TRACK_GAIN), TrackOutcome::OutOfReach];

    loudness::record_album(&db, &job, &AlbumOutcome { tracks, album: None }).await?;

    assert_eq!(owed_album_ids(&db).await?, [album]);
    Ok(())
}

#[tokio::test]
async fn a_track_that_leaves_the_library_takes_its_measurement() -> Result<(), AppError> {
    let db = library().await?;
    let id = track(&db, "Bare", "").await?;
    measure_alone(&db, id, TRACK_GAIN).await?;

    execute(&db, &format!("DELETE FROM tracks WHERE id = {id}")).await?;

    assert_eq!(measurements(&db).await?, 0);
    Ok(())
}

/// What keeps an album from being measured as one whole, one twist to its second member at a time.
#[tokio::test]
async fn an_album_is_owed_whole_only_while_every_member_is_bare_and_in_the_library()
-> Result<(), AppError> {
    let twists = [
        ("every member bare", "", true),
        (
            "a member tagged",
            "UPDATE tracks SET replaygain_album_gain = -2.0 WHERE title = 'B'",
            false,
        ),
        ("a member unhashed", "UPDATE tracks SET file_hash = NULL WHERE title = 'B'", false),
        (
            "measured whole already",
            "INSERT INTO track_loudness (track_id, file_hash, track_gain, track_peak, album_id, \
             album_tracks, album_gain, album_peak) \
             SELECT id, file_hash, -4.5, 0.5, album_id, 2, -1.5, 0.75 FROM tracks",
            false,
        ),
        ("filed outside the library", "UPDATE tracks SET folder_id = 2", false),
    ];
    for (twist, statement, expected) in twists {
        let db = library().await?;
        track(&db, "A", "Tones").await?;
        track(&db, "B", "Tones").await?;
        execute(&db, statement).await?;

        let owed = !owed_album_ids(&db).await?.is_empty();

        assert_eq!(owed, expected, "{twist}");
    }
    Ok(())
}

#[tokio::test]
async fn the_tracks_owed_alone_are_bare_and_off_any_album_measurable_whole() -> Result<(), AppError>
{
    let db = library().await?;
    let lone = track(&db, "Lone", "").await?;
    let tagged = track(&db, "Tagged", "").await?;
    let unhashed = track(&db, "Unhashed", "").await?;
    let measured = track(&db, "Measured", "").await?;
    let outside = track(&db, "Outside", "").await?;
    let mixed_tagged = track(&db, "Mixed tagged", "Mixed").await?;
    let mixed_bare = track(&db, "Mixed bare", "Mixed").await?;
    track(&db, "Whole", "Whole").await?;
    execute(
        &db,
        &format!(
            "UPDATE tracks SET replaygain_track_gain = -2.0 WHERE id IN ({tagged}, {mixed_tagged});
             UPDATE tracks SET file_hash = NULL WHERE id = {unhashed};
             UPDATE tracks SET folder_id = 2 WHERE id = {outside};"
        ),
    )
    .await?;
    measure_alone(&db, measured, TRACK_GAIN).await?;

    let owed = owed_track_ids(&db).await?;

    assert_eq!(owed, [lone, mixed_bare]);
    Ok(())
}

/// The total the progress row counts towards.
#[tokio::test]
async fn pending_counts_every_track_a_pass_will_decode() -> Result<(), AppError> {
    let db = library().await?;
    track(&db, "Whole 1", "Whole").await?;
    track(&db, "Whole 2", "Whole").await?;
    let mixed_tagged = track(&db, "Mixed tagged", "Mixed").await?;
    track(&db, "Mixed bare", "Mixed").await?;
    track(&db, "Lone", "").await?;
    execute(
        &db,
        &format!("UPDATE tracks SET replaygain_track_gain = -2.0 WHERE id = {mixed_tagged}"),
    )
    .await?;

    let pending = loudness::pending(&db).await?;

    assert_eq!(pending, Pending { albums: 1, tracks: 4 });
    Ok(())
}

#[tokio::test]
async fn an_album_s_members_come_in_album_detail_order() -> Result<(), AppError> {
    let db = library().await?;
    let d1_t2 = numbered(&db, "D1 T2", 1, Some(2)).await?;
    let d2_none = numbered(&db, "D2 none", 2, None).await?;
    let d2_t1 = numbered(&db, "D2 T1", 2, Some(1)).await?;
    let d1_t1 = numbered(&db, "D1 T1", 1, Some(1)).await?;

    let job = owed_album(&db, album_of(&db, d1_t1).await?).await?;

    let order: Vec<i64> = job.members.iter().map(|member| member.id).collect();
    assert_eq!(order, [d1_t1, d1_t2, d2_t1, d2_none]);
    Ok(())
}

#[tokio::test]
async fn queued_ids_name_the_job_each_belongs_to() -> Result<(), AppError> {
    let db = library().await?;
    track(&db, "Whole 1", "Whole").await?;
    let queued_member = track(&db, "Whole 2", "Whole").await?;
    track(&db, "Other", "Other").await?;
    let lone = track(&db, "Lone", "").await?;
    let tagged = track(&db, "Tagged", "").await?;
    execute(&db, &format!("UPDATE tracks SET replaygain_track_gain = -2.0 WHERE id = {tagged}"))
        .await?;
    let whole = album_of(&db, queued_member).await?;

    let queued = loudness::queued_jobs(&db, &[queued_member, lone, tagged]).await?;

    let albums: Vec<i64> = queued.albums.iter().map(|album| album.album_id).collect();
    let tracks: Vec<i64> = queued.tracks.iter().map(|track| track.id).collect();
    assert_eq!((albums, tracks), (vec![whole], vec![lone]));
    Ok(())
}

/// Any change to the album since it was measured strips the values its members hold, so none plays
/// a gain measured over different audio.
#[tokio::test]
async fn settle_strips_an_album_that_changed_since_it_was_measured() -> Result<(), AppError> {
    let changes = [
        (
            "a member joined",
            "UPDATE tracks SET album_id = (SELECT album_id FROM tracks WHERE title = 'A') \
             WHERE title = 'C'",
        ),
        ("a member left", "DELETE FROM tracks WHERE title = 'B'"),
        ("a member tagged", "UPDATE tracks SET replaygain_track_gain = -2.0 WHERE title = 'B'"),
        ("a member's file replaced", "UPDATE tracks SET file_hash = 'replaced' WHERE title = 'B'"),
    ];
    for (change, statement) in changes {
        let db = library().await?;
        let first = track(&db, "A", "Tones").await?;
        track(&db, "B", "Tones").await?;
        track(&db, "C", "").await?;
        measure_whole(&db, first).await?;
        execute(&db, statement).await?;

        loudness::settle(&db).await?;

        assert_eq!(rows_holding_album_values(&db).await?, 0, "{change}");
    }
    Ok(())
}

/// The common wake, where nothing moved.
#[tokio::test]
async fn settle_keeps_an_album_nothing_changed_in() -> Result<(), AppError> {
    let db = library().await?;
    let first = track(&db, "A", "Tones").await?;
    track(&db, "B", "Tones").await?;
    measure_whole(&db, first).await?;

    let settled = loudness::settle(&db).await?;

    assert_eq!((settled, rows_holding_album_values(&db).await?), (0, 2));
    Ok(())
}

/// Two albums of one size swapping members both keep their member counts, so only the album each
/// value was measured over gives the swap away.
#[tokio::test]
async fn settle_catches_members_swapped_between_albums_of_one_size() -> Result<(), AppError> {
    let db = library().await?;
    let x = track(&db, "X", "Ex").await?;
    let y = track(&db, "Y", "Why").await?;
    measure_whole(&db, x).await?;
    measure_whole(&db, y).await?;
    let (ex, why) = (album_of(&db, x).await?, album_of(&db, y).await?);
    execute(
        &db,
        &format!(
            "UPDATE tracks SET album_id = {why} WHERE id = {x};
             UPDATE tracks SET album_id = {ex} WHERE id = {y};"
        ),
    )
    .await?;

    loudness::settle(&db).await?;

    assert_eq!(rows_holding_album_values(&db).await?, 0);
    Ok(())
}

/// What reading back a file whose tags were rewritten finds: the row's own values, under `hash`.
fn read_back(title: &str, hash: &str) -> melodia_core::entities::scan::ExtractedMetadata {
    let mut meta = make_test_metadata(title);
    meta.artist = ArtistCredit::from_name("Artist");
    meta.album = None;
    meta.genres = GenreList::from_name("Rock");
    meta.file_hash = hash.to_owned();
    meta
}

async fn retag(db: &DbPool, id: i64, title: &str, hash: &str) -> Result<(), AppError> {
    let file =
        RetaggedFile { id, path: format!("/music/{title}.flac"), meta: read_back(title, hash) };
    commit_retag(db, vec![file], &TagEdit::default(), None).await?;
    Ok(())
}

#[tokio::test]
async fn a_tag_write_carries_the_measurement_onto_the_new_hash() -> Result<(), AppError> {
    let db = library().await?;
    let id = track(&db, "Bare", "").await?;
    measure_alone(&db, id, TRACK_GAIN).await?;

    retag(&db, id, "Bare", "rewritten").await?;

    assert_eq!(origin(&db, id).await?, GainOrigin::Measured);
    Ok(())
}

#[tokio::test]
async fn a_tag_write_leaves_a_stale_measurement_stale() -> Result<(), AppError> {
    let db = library().await?;
    let id = track(&db, "Bare", "").await?;
    measure_alone(&db, id, TRACK_GAIN).await?;
    execute(&db, &format!("UPDATE tracks SET file_hash = 'replaced' WHERE id = {id}")).await?;

    retag(&db, id, "Bare", "rewritten").await?;

    assert_eq!(origin(&db, id).await?, GainOrigin::Tagged);
    Ok(())
}

#[tokio::test]
async fn an_image_s_tracks_keep_their_measurements_across_a_sheet_write() -> Result<(), AppError> {
    let db = library().await?;
    let path = "/music/image.flac";
    let first = insert_test_track(&db, path, "One", "Artist", "", "Rock").await?;
    execute(
        &db,
        &format!("UPDATE tracks SET cue_track = 1, cue_start = 0, cue_end = 75 WHERE id = {first}"),
    )
    .await?;
    let second = insert_test_track(&db, path, "Two", "Artist", "", "Rock").await?;
    execute(
        &db,
        &format!(
            "UPDATE tracks SET cue_track = 2, cue_start = 75 WHERE id = {second};
             UPDATE tracks SET file_hash = 'image' WHERE file_path = '{path}';"
        ),
    )
    .await?;
    measure_alone(&db, first, TRACK_GAIN).await?;
    measure_alone(&db, second, TRACK_GAIN).await?;
    let before = loudness::hashes_on_record(&db, vec![path.to_owned()]).await?;
    execute(&db, &format!("UPDATE tracks SET file_hash = 'rewritten' WHERE file_path = '{path}'"))
        .await?;

    loudness::carry_over_rewritten(&db, before).await?;

    let origins = [origin(&db, first).await?, origin(&db, second).await?];
    assert_eq!(origins, [GainOrigin::Measured, GainOrigin::Measured]);
    Ok(())
}
