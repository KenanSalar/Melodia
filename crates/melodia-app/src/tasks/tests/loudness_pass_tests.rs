//! What a loudness pass hands out, in what order, and what its progress shows once it ends.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::{Controls, Halt, JobKey, PassEnd, Tally, WorkList, reachable_total, run};
use crate::state::fixtures::TestPlayback;
use crate::state::{LoudnessControl, MeasurePhase, MeasureProgress, ScanControl};
use melodia_core::error::AppResult;
use melodia_store::database::queries::fixtures::{fetch_one, insert_test_track};
use melodia_store::database::queries::loudness::{self, Pending};
use melodia_store::database::{DbPool, queries};
use melodia_testkit::ASSETS_DIR;

/// The Settings card's row once a pass ends: its count where a Stop or a scan left work behind,
/// and gone otherwise.
#[test]
fn a_pass_s_end_shows_what_is_left_and_why() {
    let left = |phase| Some(MeasureProgress { phase, done: 1, total: 3 });
    let gone = Some(MeasureProgress::default());
    let rows = [
        (None, 3, gone),
        (Some(Halt::Shutdown), 1, None),
        (Some(Halt::User), 1, left(MeasurePhase::Stopped)),
        (Some(Halt::User), 3, gone),
        (Some(Halt::Scan), 1, left(MeasurePhase::WaitingForScan)),
        (Some(Halt::Scan), 3, gone),
        (Some(Halt::Off), 1, gone),
        (Some(Halt::Failed), 1, gone),
    ];

    for (halt, done, expected) in rows {
        let end = PassEnd { halt, done, total: 3 };
        assert_eq!(end.progress(), expected, "{halt:?} at {done} of 3");
    }
}

/// A library folder at `/music`, folder 1, whose files are never read.
async fn library() -> AppResult<DbPool> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    Ok(db)
}

/// An untagged track on `album`, or on none where it is empty.
async fn track(db: &DbPool, title: &str, album: &str) -> AppResult<i64> {
    insert_test_track(db, &format!("/music/{title}.flac"), title, "Artist", album, "Rock").await
}

async fn album_of(db: &DbPool, track: i64) -> AppResult<JobKey> {
    let album = fetch_one(db, &format!("SELECT album_id FROM tracks WHERE id = {track}")).await?;
    Ok(JobKey::Album(album))
}

/// A pass's work list, and the queue window's half of the channel it reads queued ids from.
fn work_list(db: &DbPool) -> (WorkList, watch::Sender<Vec<i64>>) {
    let (queue, ids) = watch::channel(Vec::new());
    (WorkList::new(db.clone(), ids), queue)
}

async fn next_key(work: &mut WorkList, in_flight: &[JobKey]) -> AppResult<Option<JobKey>> {
    let job = work.next(in_flight, &HashSet::new()).await?;
    Ok(job.map(|job| job.key()))
}

/// Every job `work` hands out with none in flight, in the order it hands them.
async fn drain(work: &mut WorkList, out_of_reach: &HashSet<JobKey>) -> AppResult<Vec<JobKey>> {
    let mut keys = Vec::new();
    while let Some(job) = work.next(&[], out_of_reach).await? {
        keys.push(job.key());
    }
    Ok(keys)
}

/// During a long backlog what is about to play is measured next, and the keyset passes it by when
/// it gets there.
#[tokio::test]
async fn the_queue_s_album_is_handed_out_first_and_only_once() -> AppResult<()> {
    let db = library().await?;
    let backlog = track(&db, "A", "Backlog").await?;
    let queued = track(&db, "B", "Queued").await?;
    let (mut work, queue) = work_list(&db);
    queue.send_replace(vec![queued]);

    let handed_out = drain(&mut work, &HashSet::new()).await?;

    assert_eq!(handed_out, [album_of(&db, queued).await?, album_of(&db, backlog).await?]);
    Ok(())
}

/// The queue's copy of a job the keyset already took can wait behind other queued work, and once
/// the keyset's copy lands it must not be handed out again.
#[tokio::test]
async fn a_job_that_landed_is_not_handed_out_again_from_the_queue() -> AppResult<()> {
    let db = library().await?;
    let lone = track(&db, "Lone", "").await?;
    let (mut work, queue) = work_list(&db);
    let taken = next_key(&mut work, &[]).await?;
    let arrived = track(&db, "Arrived", "Arrived").await?;
    let arrived_album = album_of(&db, arrived).await?;
    queue.send_replace(vec![arrived, lone]);
    let ahead = next_key(&mut work, &[JobKey::Track(lone)]).await?;
    work.landed(JobKey::Track(lone));

    let after = next_key(&mut work, &[arrived_album]).await?;

    assert_eq!([taken, ahead, after], [Some(JobKey::Track(lone)), Some(arrived_album), None]);
    Ok(())
}

/// A job found out of reach this session waits for the next launch, so the keyset passes it over
/// and the total the row shows stops counting its tracks.
#[tokio::test]
async fn a_job_out_of_reach_is_passed_over_and_off_the_total() -> AppResult<()> {
    let db = library().await?;
    let gone = track(&db, "Gone 1", "Gone").await?;
    track(&db, "Gone 2", "Gone").await?;
    let lone = track(&db, "Lone", "").await?;
    let out_of_reach = HashSet::from([album_of(&db, gone).await?]);
    let (mut work, _queue) = work_list(&db);

    let handed_out = drain(&mut work, &out_of_reach).await?;

    let tally = Tally { total: 3, ..Tally::default() };
    assert_eq!((handed_out, reachable_total(&tally, &work)), (vec![JobKey::Track(lone)], 1));
    Ok(())
}

/// A pass over `fx`'s library with `ReplayGain` and measuring both on.
fn measuring(fx: &TestPlayback) -> Arc<Controls> {
    fx.ctx.engine.set_replaygain_enabled(true);
    fx.ctx.engine.set_replaygain_measured(true);
    Arc::new(Controls {
        db: fx.ctx.db.clone(),
        loudness: Arc::new(LoudnessControl::new()),
        scan: Arc::new(ScanControl::new(&CancellationToken::new())),
        playback: fx.ctx.clone(),
        shutdown: CancellationToken::new(),
    })
}

/// A playable track on no album, beside the ones [`TestPlayback::stage_playable`] staged.
async fn stage_lone(fx: &TestPlayback) -> AppResult<()> {
    let path = fx.tmp.path().join("music").join("lone.mp3");
    std::fs::copy(std::path::Path::new(ASSETS_DIR).join("silence.mp3"), &path)?;
    insert_test_track(&fx.ctx.db, &path.to_string_lossy(), "Lone", "Artist", "", "Rock").await?;
    Ok(())
}

/// A backlog measured through, an album whole and a track alone, leaves the next launch nothing to
/// do. Silence measures to nothing to level, which is recorded all the same.
#[tokio::test]
async fn a_pass_leaves_nothing_owed() -> AppResult<()> {
    let fx = TestPlayback::empty().await?;
    fx.stage_playable(2).await?;
    stage_lone(&fx).await?;
    let controls = measuring(&fx);

    run(&controls, &mut HashSet::new()).await;

    assert_eq!(loudness::pending(&fx.ctx.db).await?, Pending::default());
    Ok(())
}

/// A scan reads the same disks and needs the one writer, so a pass meeting one steps aside with
/// its work shown waiting.
#[tokio::test]
async fn a_pass_that_meets_a_scan_waits_for_it() -> AppResult<()> {
    let fx = TestPlayback::empty().await?;
    fx.stage_playable(2).await?;
    let controls = measuring(&fx);
    let _scan = controls.scan.hold_busy();

    let end = run(&controls, &mut HashSet::new()).await;

    let waiting = MeasureProgress { phase: MeasurePhase::WaitingForScan, done: 0, total: 2 };
    assert_eq!(end.progress(), Some(waiting));
    Ok(())
}
