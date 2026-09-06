//! The periodic save, which is what survives a SIGKILL.
//!
//! Persistence is deliberately off the `PlayerAction` list, so nothing else in the tree writes
//! what this does: the resume position onto the row, and the queue plus the station tuned over it
//! into `queue.json`. Every failure inside it is a warning, so a half that stops writing costs a
//! user their place and reports nothing at all.

use std::path::PathBuf;

use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;
use melodia_engine::player::engine::types::{PersistableQueue, PersistedPlayback};
use melodia_store::database::queries::fixtures::insert_test_track;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A library with one track row, and the `queue.json` the monitor writes beside it.
struct Fixture {
    db: DbPool,
    queue_path: PathBuf,
    track_id: i64,
    _tmp: TempDir,
}

impl Fixture {
    async fn new() -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let db = DbPool::test_pool().await?;
        queries::folder::insert_folder(&db, "/music", true).await?;
        let track_id =
            insert_test_track(&db, "/music/song.mp3", "Song", "Artist", "Album", "Rock").await?;
        Ok(Self {
            db,
            queue_path: tmp.path().join("queue.json"),
            track_id,
            _tmp: tmp,
        })
    }

    async fn last_position(&self) -> Result<i64, AppError> {
        let position =
            sqlx::query_scalar::<_, i64>("SELECT last_position FROM tracks WHERE id = ?")
                .bind(self.track_id)
                .fetch_one(self.db.read())
                .await?;
        Ok(position)
    }

    /// Read the file back the way the startup restore does, so a shape only this test can parse
    /// would fail here too.
    fn written_playback(&self) -> TestResult<PersistedPlayback> {
        Ok(serde_json::from_str(&std::fs::read_to_string(&self.queue_path)?)?)
    }
}

/// A queue of one, over the fixture's own row.
fn playback(station_id: Option<i64>, track_id: i64) -> PersistedPlayback {
    PersistedPlayback {
        queue: PersistableQueue {
            track_ids: vec![track_id],
            current_index: 0,
        },
        station_id,
    }
}

/// The resume position, which is the only thing that puts a user back where they were after a
/// crash — the clean-exit snapshot never ran.
#[tokio::test]
async fn the_snapshot_position_lands_on_the_track_row() -> TestResult {
    let fixture = Fixture::new().await?;

    persist(
        &fixture.db,
        &fixture.queue_path,
        PlaybackSnapshot {
            track: Some((fixture.track_id, 42_000)),
            playback: playback(None, fixture.track_id),
        },
    )
    .await;

    assert_eq!(fixture.last_position().await?, 42_000);
    Ok(())
}

/// A tick can land with nothing on the deck — the queue is seated, playback is stopped, a station
/// is tuned. The row write is guarded on a playing track and the file write is not, because what
/// a restart owes back is the queue either way.
#[tokio::test]
async fn a_snapshot_with_nothing_playing_still_writes_the_queue_file() -> TestResult {
    let fixture = Fixture::new().await?;

    persist(
        &fixture.db,
        &fixture.queue_path,
        PlaybackSnapshot {
            track: None,
            playback: playback(None, fixture.track_id),
        },
    )
    .await;

    assert_eq!(fixture.last_position().await?, 0, "there was no track to write a position for");
    assert_eq!(fixture.written_playback()?.queue.track_ids, [fixture.track_id]);
    Ok(())
}

/// The reason `queue.json` is one file rather than two. A station leaves the queue seated
/// underneath it, so a restart owes both halves — write only the queue and the station comes back
/// stopped with a library the user never asked to see.
#[tokio::test]
async fn the_station_seated_under_the_queue_is_written_with_it() -> TestResult {
    let fixture = Fixture::new().await?;

    persist(
        &fixture.db,
        &fixture.queue_path,
        PlaybackSnapshot {
            track: None,
            playback: playback(Some(7), fixture.track_id),
        },
    )
    .await;

    let written = fixture.written_playback()?;
    assert_eq!(written.station_id, Some(7), "the station on the deck");
    assert_eq!(written.queue.track_ids, [fixture.track_id], "and the queue underneath it");
    Ok(())
}
