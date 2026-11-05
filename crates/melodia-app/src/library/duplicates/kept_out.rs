//! The files Remove kept out of the library, and Bring Back, which returns one through the import a
//! drop takes: the import lifts the exclusion inside its own transaction, so a file comes back
//! with no rescan.

use melodia_core::entities::duplicates::KeptOutFile;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

use crate::library::import::import_files;
use crate::state::AppState;

/// Files parsed and committed together. A page bounds the tags held between the parse and the
/// commit, and keeps each import's write transaction short.
const BRING_BACK_PAGE: usize = 500;

/// What a Bring Back did, for its toast.
#[derive(Clone, Copy, Debug, Default)]
pub struct BroughtBack {
    pub returned: usize,
    /// Files gone from disk, on a drive not connected, or unreadable: each stays kept out.
    pub not_returned: usize,
}

pub async fn count(db: &DbPool) -> Result<i64, AppError> {
    queries::excluded_files::count(db).await
}

pub async fn ids(db: &DbPool) -> Result<Vec<i64>, AppError> {
    queries::excluded_files::ids(db).await
}

/// The files behind `ids` as they read now, for the rows on screen.
pub async fn files(db: &DbPool, ids: Vec<i64>) -> Result<Vec<KeptOutFile>, AppError> {
    queries::excluded_files::files(db, ids).await
}

pub async fn paths(db: &DbPool) -> Result<Vec<String>, AppError> {
    queries::excluded_files::paths(db).await
}

/// Returns the files behind `ids` to the library. An id lifted since, by the watcher or another
/// Bring Back, counts as neither returned nor not.
pub async fn bring_back(state: &AppState, ids: &[i64]) -> Result<BroughtBack, AppError> {
    let mut brought = BroughtBack::default();
    let mut outcome = Ok(());
    for page in ids.chunks(BRING_BACK_PAGE) {
        match bring_back_page(state, page).await {
            Ok(page) => {
                brought.returned += page.returned;
                brought.not_returned += page.not_returned;
            }
            Err(e) => {
                outcome = Err(e);
                break;
            }
        }
    }
    // Before a failure returns too, so the pages that committed still reach every list.
    if brought.returned > 0 {
        state.library_changed.bump();
    }
    outcome.map(|()| brought)
}

/// [`bring_back`] on every kept-out file.
pub async fn bring_all_back(state: &AppState) -> Result<BroughtBack, AppError> {
    let ids = ids(&state.db).await?;
    bring_back(state, &ids).await
}

async fn bring_back_page(state: &AppState, ids: &[i64]) -> Result<BroughtBack, AppError> {
    let paths: Vec<String> =
        files(&state.db, ids).await?.into_iter().map(|file| file.path).collect();
    if paths.is_empty() {
        return Ok(BroughtBack::default());
    }
    import_files(&state.db, &state.paths.artwork_dir, &state.cover_cache, &paths).await?;
    // Read back rather than taken from the insert count: a file whose bytes match a row whose own
    // file is gone takes that row back and inserts nothing.
    let still_out = files(&state.db, ids).await?.len().min(paths.len());
    Ok(BroughtBack { returned: paths.len() - still_out, not_returned: still_out })
}
