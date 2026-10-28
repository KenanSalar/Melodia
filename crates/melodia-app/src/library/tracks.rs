use melodia_core::entities::track;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// How many tracks the library holds, which a filtered Songs list states beside its matches.
pub async fn count(db: &DbPool) -> Result<i64, AppError> {
    queries::track::count_tracks(db).await
}

/// The projection Now Playing's technical chips render: the list projections lack those columns,
/// and a full `Track` would pull many the chips never read.
pub async fn get_track_meta(db: &DbPool, id: i64) -> Result<Option<track::TrackMeta>, AppError> {
    queries::track::get_track_meta(db, id).await
}

/// In the order asked. Ids with no row are dropped, so a shorter result is a queue that outlived a
/// delete rather than an error.
pub async fn get_track_links(db: &DbPool, ids: &[i64]) -> Result<Vec<track::TrackLinks>, AppError> {
    queries::track::get_track_links_by_ids(db, ids).await
}

/// Open the OS file manager at the folder holding the track.
pub async fn reveal_in_file_manager(db: &DbPool, track_id: i64) -> Result<(), AppError> {
    let file_path = queries::track::get_track_file_path(db, track_id)
        .await?
        .ok_or_else(|| AppError::not_found("Track", track_id))?;

    let folder = std::path::Path::new(&file_path)
        .parent()
        .ok_or_else(|| AppError::io_other(format!("track has no parent folder: {file_path}")))?
        .to_path_buf();

    reveal_folder(folder).await
}

/// Open the OS file manager at `folder`, on the blocking pool since the existence check is a `stat`.
///
/// **Detached, never `open::that`**, for the reason `ui::launcher` gives: `that` `waitpid`s the
/// launcher, and a file manager that does not daemonise holds a blocking-pool slot for as long as
/// the user leaves its window open.
pub async fn reveal_folder(folder: std::path::PathBuf) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        if !folder.is_dir() {
            return Err(AppError::io_other(format!(
                "folder no longer exists: {}",
                folder.display()
            )));
        }
        open::that_detached(&folder)?;
        Ok(())
    })
    .await
    .map_err(|e| AppError::io("reveal task join failed", e))??;

    Ok(())
}
