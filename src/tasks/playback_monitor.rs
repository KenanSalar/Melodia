use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::database::{DbPool, queries};
use crate::player::handlers::{
    PlaybackMonitorContext, PlaybackSnapshot, SnapshotSink, spawn_playback_monitor,
};
use crate::state::AppState;
use crate::tasks::TaskSpawner;

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let db = state.db.clone();
    let queue_path = state.paths.queue_path.clone();
    let save: SnapshotSink = Arc::new(move |snapshot| {
        let db = db.clone();
        let queue_path = queue_path.clone();
        Box::pin(async move { persist(&db, &queue_path, snapshot).await })
    });

    spawn_playback_monitor(
        &spawner.tracker,
        PlaybackMonitorContext {
            shutdown_token: spawner.shutdown.clone(),
            player_state: state.player_state.clone(),
            engine: state.engine.clone(),
            sinks: state.sinks.clone(),
            position_tx: state.position_tx.clone(),
            save,
        },
    );
    log::info!("Playback monitor started");
}

/// Write one periodic snapshot down: the resume position into the database, the queue and
/// whatever station sits under it into `queue.json`.
///
/// Survives SIGKILL and a crash; the authoritative final snapshot is still `shutdown`'s on a
/// clean exit. Both failures are warnings — the next tick writes the same thing 30 s later.
async fn persist(db: &DbPool, queue_path: &Path, snapshot: PlaybackSnapshot) {
    if let Some((track_id, position_ms)) = snapshot.track
        && let Err(e) = queries::track::update_last_position(
            db,
            track_id,
            i64::try_from(position_ms).unwrap_or(i64::MAX),
        )
        .await
    {
        log::warn!("periodic save: update_last_position {track_id}: {e}");
    }

    let path: PathBuf = queue_path.to_path_buf();
    let playback = snapshot.playback;
    let join = tokio::task::spawn_blocking(move || {
        crate::utils::atomic_file::write_json_sync(&path, &playback)
    })
    .await;
    match join {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::warn!("periodic save: write queue.json: {e}"),
        Err(e) => log::warn!("periodic save: spawn_blocking: {e}"),
    }
}
