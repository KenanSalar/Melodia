use crate::state::AppState;
use melodia_core::entities::track;
use melodia_core::error::AppError;
use melodia_store::database::queries;

pub async fn get_tracks(state: &AppState) -> Result<Vec<track::TrackListRow>, AppError> {
    queries::track::get_all_tracks_for_list(&state.db).await
}

/// Fetch the [`track::TrackMeta`] projection for a single id — the 8
/// columns the full-screen Now Playing view's technical-metadata chips
/// (codec / bitrate / sample rate / bit depth / channels / year / genre)
/// render. Returns `None` for a missing id. The slimmer list/summary
/// projections don't carry those columns, but a full `Track` would pull
/// 33 more the chips never read.
pub async fn get_track_meta(
    state: &AppState,
    id: i64,
) -> Result<Option<track::TrackMeta>, AppError> {
    queries::track::get_track_meta(&state.db, id).await
}

/// Open the OS file manager at the folder containing the given track,
/// backing the track-row "Open Containing Folder" context-menu action.
///
/// Resolves the track's `file_path` from the DB, then hands its parent
/// directory to the platform default handler via the `open` crate
/// (`xdg-open` on Linux, `open` on macOS, PowerShell `Start-Process` with
/// an `explorer.exe` fallback on Windows). The directory-existence check
/// and the launcher call both run on the blocking pool — `is_dir` is a
/// `stat` syscall and `open::that` spawns and waits on a child process.
pub async fn reveal_in_file_manager(state: &AppState, track_id: i64) -> Result<(), AppError> {
    let file_path = queries::track::get_track_file_path(&state.db, track_id)
        .await?
        .ok_or_else(|| AppError::not_found("Track", track_id))?;

    let folder = std::path::Path::new(&file_path)
        .parent()
        .ok_or_else(|| AppError::io_other(format!("track has no parent folder: {file_path}")))?
        .to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        if !folder.is_dir() {
            return Err(AppError::io_other(format!(
                "folder no longer exists: {}",
                folder.display()
            )));
        }
        open::that(&folder)?;
        Ok(())
    })
    .await
    .map_err(|e| AppError::io_other(format!("reveal task join failed: {e}")))??;

    Ok(())
}
