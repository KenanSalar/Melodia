//! Tests for the coalescing write-back, against real fixtures copied out of `test-assets/` into
//! a `TempDir`. Never write to the checked-in asset.
//!
//! This is the one task in the tree that rewrites the user's own music. lofty re-serializes the
//! whole tag, so a star on a 32 MB MP3 rewrites 32 MB, and what decides both *whether* that
//! happens and *which value* it writes is the map and the quiet period below. Two of the three
//! properties here are ones a user would call a violation rather than a bug: a four-star rating
//! landing in the file as one, and a write happening after the switch was turned off.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lofty::file::TaggedFileExt;
use tempfile::TempDir;
use tokio::sync::mpsc::{self, UnboundedSender};

use super::*;
use melodia_artwork::media::image::artwork;
use melodia_core::error::AppError;
use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::database::queries;
use melodia_store::database::queries::fixtures::insert_test_track;
use melodia_store::media::ingest::{metadata, rating_tags};
use melodia_testkit::ASSETS_DIR;

/// A staged library: one `Writeback` over a `test_pool`, plus the temp root its files live under.
struct Fixture {
    writeback: Writeback,
    db: DbPool,
    tmp: TempDir,
    switch: SharedFlag,
}

impl Fixture {
    async fn new(write_to_tags: bool) -> Result<Self, AppError> {
        let db = DbPool::test_pool().await?;
        let tmp = TempDir::new()?;
        queries::folder::insert_folder(&db, &tmp.path().to_string_lossy(), true).await?;

        let artwork_dir = tmp.path().join("artwork");
        std::fs::create_dir(&artwork_dir)?;
        let switch = SharedFlag::new(write_to_tags);

        Ok(Self {
            writeback: Writeback {
                db: db.clone(),
                artwork_dir,
                cover_cache: artwork::new_cover_cache(),
                self_writes: Arc::new(SelfWrites::default()),
                write_to_tags: switch.clone(),
            },
            db,
            tmp,
            switch,
        })
    }

    /// Copy a fixture in under `name` and give it a row. FLAC because its Vorbis comments hold a
    /// rating without the ID3 popularimeter mapping being in the way.
    async fn track(&self, name: &str) -> Result<(i64, PathBuf), AppError> {
        let dst = self.tmp.path().join(name);
        std::fs::copy(PathBuf::from(ASSETS_DIR).join("silence.flac"), &dst)?;
        let id =
            insert_test_track(&self.db, &dst.to_string_lossy(), name, "Artist", "Album", "Rock")
                .await?;
        Ok((id, dst))
    }
}

/// What the file itself says, which is the only thing this task exists to change. The row is
/// written a layer up by `library::ratings` and proves nothing here.
fn stars_in_file(path: &Path) -> Option<i32> {
    let tagged = metadata::read_tags(path, metadata::TagScope::TagsOnly).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    rating_tags::stars_from_tag(tag)
}

/// Drive `run` to completion over `events`, ending it by dropping the sender so the final flush
/// is the one on the receive arm. No clock involved, which is what makes the value assertions
/// separable from the timing one below.
async fn run_to_completion(writeback: &Writeback, send: impl FnOnce(&UnboundedSender<(i64, i32)>)) {
    let (tx, rx) = mpsc::unbounded_channel();
    send(&tx);
    drop(tx);
    run(rx, CancellationToken::new(), writeback.clone()).await;
}

/// The star strip is five adjacent click targets, so picking four stars sends one, two, three and
/// four on the way. Only where the finger stopped may reach the file.
#[tokio::test]
async fn a_walk_across_the_star_strip_writes_only_where_the_finger_stopped() -> Result<(), AppError>
{
    let fixture = Fixture::new(true).await?;
    let (id, path) = fixture.track("walk.flac").await?;

    run_to_completion(&fixture.writeback, |tx| {
        for stars in 1..=4 {
            assert!(tx.send((id, stars)).is_ok());
        }
    })
    .await;

    assert_eq!(
        stars_in_file(&path),
        Some(4),
        "the file has to hold the rating the user settled on, not the first one they crossed"
    );
    Ok(())
}

/// Two tracks rated differently in one burst are two writes, not one applied twice. The regroup
/// by value is what lets an album share a single pass, and it is also what could cross the wires.
#[tokio::test]
async fn a_burst_over_two_tracks_gives_each_its_own_rating() -> Result<(), AppError> {
    let fixture = Fixture::new(true).await?;
    let (first, first_path) = fixture.track("first.flac").await?;
    let (second, second_path) = fixture.track("second.flac").await?;

    run_to_completion(&fixture.writeback, |tx| {
        assert!(tx.send((first, 2)).is_ok());
        assert!(tx.send((second, 5)).is_ok());
        assert!(tx.send((first, 3)).is_ok());
    })
    .await;

    assert_eq!(stars_in_file(&first_path), Some(3), "the later value for a track is its value");
    assert_eq!(stars_in_file(&second_path), Some(5), "and it must not reach the other track");
    Ok(())
}

/// The switch is read at flush, not at send, because it can move while a burst is in flight. Both
/// directions matter and they fail differently: read at send, an install switched off mid-burst
/// still writes, which is the behaviour a user would call a violation.
#[tokio::test]
async fn the_switch_is_read_when_the_write_happens_not_when_the_star_was_clicked()
-> Result<(), AppError> {
    for (started_on, expected) in [(true, None), (false, Some(3))] {
        let fixture = Fixture::new(started_on).await?;
        let (id, path) = fixture.track("switched.flac").await?;

        let (tx, rx) = mpsc::unbounded_channel();
        assert!(tx.send((id, 3)).is_ok());
        drop(tx);

        // Flipped after the send is queued and before the loop ever looks at it.
        fixture.switch.set(!started_on);
        run(rx, CancellationToken::new(), fixture.writeback.clone()).await;

        assert_eq!(
            stars_in_file(&path),
            expected,
            "started_on={started_on}: the answer that matters is the switch at the moment of writing"
        );
    }
    Ok(())
}

/// A burst that arrives with the switch off is dropped rather than held. Keeping it would write
/// the stars later, when the user has said no and moved on.
#[tokio::test]
async fn a_burst_refused_by_the_switch_is_not_kept_for_later() -> Result<(), AppError> {
    let fixture = Fixture::new(false).await?;
    let (id, path) = fixture.track("refused.flac").await?;

    let mut pending = HashMap::from([(id, 5)]);
    flush(&fixture.writeback, &mut pending).await;
    assert!(pending.is_empty(), "a refused burst is discarded, not deferred");

    // Turning it back on must not resurrect what was refused while it was off.
    fixture.switch.set(true);
    flush(&fixture.writeback, &mut pending).await;
    assert_eq!(stars_in_file(&path), None, "nothing was held to write once the switch came back");
    Ok(())
}

/// The quiet period is what turns the walk into one write. Nothing may reach the file while the
/// clicking is still going.
///
/// Deliberately not `start_paused`: `test_pool` is a single connection onto `sqlite::memory:`, so
/// the database lives in that connection, and a clock that auto-advances past sqlx's idle and
/// lifetime timers has the pool reap it out from under the flush.
#[tokio::test]
async fn nothing_is_written_until_the_clicking_stops() -> Result<(), AppError> {
    let fixture = Fixture::new(true).await?;
    let (id, path) = fixture.track("quiet.flac").await?;

    let (tx, rx) = mpsc::unbounded_channel();
    let looping = tokio::spawn(run(rx, CancellationToken::new(), fixture.writeback.clone()));
    assert!(tx.send((id, 4)).is_ok());

    // Well inside `QUIET_PERIOD`, and read straight off the disk so the assertion needs no turn
    // of the runtime that could let the timer fire.
    tokio::time::sleep(QUIET_PERIOD / 8).await;
    assert_eq!(stars_in_file(&path), None, "a rating still moving must not have been written");

    drop(tx);
    assert!(looping.await.is_ok(), "the loop must return rather than panic once its senders go");
    assert_eq!(stars_in_file(&path), Some(4), "and it has to land once the clicking stops");
    Ok(())
}

/// The exit drain writes what the three-second shutdown budget can cover and no more, because a
/// tag write is a whole-file rewrite on a pool nothing can cancel: overrunning does not cost a
/// tag, it costs a track rewritten halfway. Both sides of the cap, since a floor would pass
/// however the budget was wired.
#[tokio::test]
async fn the_exit_drain_writes_up_to_its_budget_and_stops() -> Result<(), AppError> {
    for over_budget in [false, true] {
        let queued = if over_budget {
            SHUTDOWN_FLUSH_MAX + 4
        } else {
            SHUTDOWN_FLUSH_MAX - 3
        };
        let fixture = Fixture::new(true).await?;

        let mut paths = Vec::with_capacity(queued);
        let mut pending = HashMap::new();
        for n in 0..queued {
            let (id, path) = fixture.track(&format!("track-{n}.flac")).await?;
            pending.insert(id, 5);
            paths.push(path);
        }

        flush_at_exit(&fixture.writeback, &mut pending).await;

        let written = paths.iter().filter(|path| stars_in_file(path).is_some()).count();
        let expected = queued.min(SHUTDOWN_FLUSH_MAX);
        assert_eq!(
            written, expected,
            "over_budget={over_budget}: the drain pays for {expected} of {queued}, and the rows \
             keep their stars either way"
        );
    }
    Ok(())
}
