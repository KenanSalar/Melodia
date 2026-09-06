//! Tests for the batched play-count writer.
//!
//! Play count and `last_played` are the two columns Favorites' Most Played strip, the whole
//! Recently Played page and the stat-dependent smart playlists rank on, and they accumulate over
//! the life of an install. Nothing rebuilds them: a batch dropped here is listening history gone,
//! where a lost tag or a lost thumbnail is one rescan away. So the properties worth holding are
//! about not losing a batch and not writing one twice, and about the bump that tells those three
//! views to re-read landing on the far side of the write.

use tokio::sync::mpsc;

use super::*;
use melodia_core::error::AppError;
use melodia_store::database::queries;
use melodia_store::database::queries::fixtures::insert_test_track;

/// A pool with one library folder and one track, which is all any of these need: the subject is
/// the batch, not the row.
async fn seed(db: &DbPool) -> Result<i64, AppError> {
    queries::folder::insert_folder(db, "/music", true).await?;
    insert_test_track(db, "/music/a.mp3", "A", "Artist", "Album", "Rock").await
}

async fn counts(db: &DbPool, id: i64) -> Result<(i64, i64, Option<String>), AppError> {
    let row: (i64, i64, Option<String>) =
        sqlx::query_as("SELECT play_count, skip_count, last_played FROM tracks WHERE id = ?")
            .bind(id)
            .fetch_one(db.read())
            .await?;
    Ok(row)
}

#[test]
fn record_stacks_repeats_and_keeps_plays_apart_from_skips() {
    let mut plays = HashMap::new();
    let mut skips = HashMap::new();

    for event in [
        PlayCountEvent::Play(1),
        PlayCountEvent::Play(1),
        PlayCountEvent::Skip(1),
        PlayCountEvent::Play(2),
    ] {
        record(&mut plays, &mut skips, event);
    }

    assert_eq!(plays.get(&1), Some(&2), "a track played twice in one window owes two counts");
    assert_eq!(plays.get(&2), Some(&1));
    assert_eq!(skips.get(&1), Some(&1), "the same track can be played and skipped in one window");
    assert_eq!(skips.len(), 1, "a play must not seed a row in the skip map");
}

/// The map is drained by the write, so the batch cannot be replayed. A flusher that left it
/// populated would add the same counts again on the next tick, which reads as a user who played
/// the track twice.
#[tokio::test]
async fn a_flushed_batch_is_written_once_and_cannot_land_again() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let stats = Signal::new();

    let mut plays = HashMap::from([(id, 3)]);
    let mut skips = HashMap::new();
    flush(&db, &mut plays, &mut skips, &stats).await;

    assert_eq!(counts(&db, id).await?.0, 3);
    assert!(plays.is_empty(), "the batch has been written and must not be holdable a second time");

    flush(&db, &mut plays, &mut skips, &stats).await;
    assert_eq!(counts(&db, id).await?.0, 3, "a second flush over a drained map must write nothing");
    Ok(())
}

/// `last_played` is Recently Played's whole ordering key, so a play flush owes it alongside the
/// count. A skip does not: nothing ranks by skip count, and stamping it would put a skipped track
/// at the top of Recently Played.
#[tokio::test]
async fn a_play_stamps_last_played_and_a_skip_does_not() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let stats = Signal::new();

    let mut skips = HashMap::from([(id, 1)]);
    flush(&db, &mut HashMap::new(), &mut skips, &stats).await;
    let (plays_after_skip, skips_after_skip, stamped_by_skip) = counts(&db, id).await?;
    assert_eq!((plays_after_skip, skips_after_skip), (0, 1));
    assert!(stamped_by_skip.is_none(), "a skip is not a listen and must not enter Recently Played");

    let mut plays = HashMap::from([(id, 1)]);
    flush(&db, &mut plays, &mut HashMap::new(), &stats).await;
    assert!(counts(&db, id).await?.2.is_some(), "a play is what Recently Played orders on");
    Ok(())
}

/// The two signals are split so a per-song flush does not imply the library moved. Bumping the
/// stats channel for a skip burst would cost all three subscribers a re-fetch for a number none
/// of them reads.
#[tokio::test]
async fn a_skip_only_flush_writes_the_row_without_waking_the_stats_subscribers()
-> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let stats = Signal::new();
    let rx = stats.subscribe();
    let before = *rx.borrow();

    let mut skips = HashMap::from([(id, 2)]);
    flush(&db, &mut HashMap::new(), &mut skips, &stats).await;

    assert_eq!(counts(&db, id).await?.1, 2, "the skip still has to be written");
    assert_eq!(*rx.borrow(), before, "no view ranks by skip count, so nothing owes a re-read");
    Ok(())
}

/// One flush is one bump however many rows rode in it, and an empty flush is none at all.
#[tokio::test]
async fn a_play_flush_bumps_the_stats_channel_once_per_batch() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let second = insert_test_track(&db, "/music/b.mp3", "B", "Artist", "Album", "Rock").await?;
    let stats = Signal::new();
    let rx = stats.subscribe();
    let before = *rx.borrow();

    let mut plays = HashMap::from([(id, 1), (second, 4)]);
    flush(&db, &mut plays, &mut HashMap::new(), &stats).await;
    assert_eq!(*rx.borrow(), before + 1, "the batch is one ranking change, not one per row");

    flush(&db, &mut HashMap::new(), &mut HashMap::new(), &stats).await;
    assert_eq!(*rx.borrow(), before + 1, "an empty flush changed no ranking and owes no bump");
    Ok(())
}

/// The bump is what sends Favorites, Recently Played and the stat-dependent smart playlists back
/// to the database. Firing it for a write that did not land shows all three a count that was
/// never stored, and nothing corrects them until the next successful flush.
#[tokio::test]
async fn a_write_that_failed_does_not_announce_itself() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let stats = Signal::new();
    let rx = stats.subscribe();
    let before = *rx.borrow();

    // Closing the pool is the only failure this suite can stage that the schema cannot prevent.
    db.write().close().await;

    let mut plays = HashMap::from([(id, 1)]);
    flush(&db, &mut plays, &mut HashMap::new(), &stats).await;

    assert_eq!(
        *rx.borrow(),
        before,
        "a failed batch must not wake a subscriber that would re-read"
    );
    assert!(plays.is_empty(), "a failed batch is dropped rather than retried, and says so here");
    Ok(())
}

/// Shutdown cancels before the loop has necessarily received anything, so the drain rather than
/// the `recv` arm is what has to see a queued burst. Cancelling first is the interesting order:
/// it is the one where every event is still in the channel.
#[tokio::test]
async fn the_shutdown_drain_writes_what_never_reached_the_map() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let stats = Signal::new();
    let (tx, rx) = mpsc::unbounded_channel();

    for event in [
        PlayCountEvent::Play(id),
        PlayCountEvent::Play(id),
        PlayCountEvent::Skip(id),
    ] {
        assert!(tx.send(event).is_ok(), "the receiver is alive until `run` takes it");
    }

    let shutdown = CancellationToken::new();
    shutdown.cancel();
    run(rx, shutdown, db.clone(), stats).await;

    let (plays, skips, _) = counts(&db, id).await?;
    assert_eq!((plays, skips), (2, 1), "a burst still in the channel at exit is history too");
    Ok(())
}

/// The other way the loop ends: every sender gone. It owes the same final flush, because the
/// events are already in the map by then and nothing else will write them.
#[tokio::test]
async fn a_dropped_sender_flushes_before_the_loop_returns() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let stats = Signal::new();
    let (tx, rx) = mpsc::unbounded_channel();

    assert!(tx.send(PlayCountEvent::Play(id)).is_ok());
    drop(tx);
    run(rx, CancellationToken::new(), db.clone(), stats).await;

    assert_eq!(counts(&db, id).await?.0, 1);
    Ok(())
}

/// Neither exit path is the normal one: a session that plays for hours must write as it goes, on
/// the interval alone. `tokio::time::interval` yields its first tick immediately, so the first
/// batch lands as soon as the channel goes quiet rather than a whole `FLUSH_INTERVAL` later, and
/// the happy path here costs no wall clock.
///
/// Deliberately not `start_paused`, which every timed test reaching for a `test_pool` has to
/// resist: the pool is one connection onto `sqlite::memory:`, so the database lives *in* that
/// connection, and a clock that auto-advances past sqlx's idle and lifetime timers has the pool
/// reap it. What comes back is a pool timeout rather than anything about play counts.
#[tokio::test]
async fn the_loop_writes_on_its_own_tick_without_waiting_for_shutdown() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let id = seed(&db).await?;
    let stats = Signal::new();
    let (tx, rx) = mpsc::unbounded_channel();

    let mut bumped = stats.subscribe();
    let shutdown = CancellationToken::new();
    let looping = tokio::spawn(run(rx, shutdown.clone(), db.clone(), stats));

    assert!(tx.send(PlayCountEvent::Play(id)).is_ok());

    // Waited for rather than slept past: the bump lands after the write, so it is the one signal
    // that says the flush finished. Reading the row here instead would race the loop for
    // `test_pool`'s single connection, which under a paused clock resolves as a pool timeout.
    let flushed = tokio::time::timeout(FLUSH_INTERVAL * 5, bumped.changed()).await;
    assert!(
        flushed.is_ok(),
        "the tick has to flush on its own, with nothing cancelled and no sender dropped"
    );

    shutdown.cancel();
    assert!(looping.await.is_ok(), "the loop must return rather than panic on cancellation");
    assert_eq!(counts(&db, id).await?.0, 1, "the tick wrote it, and the exit flush must not again");
    Ok(())
}
