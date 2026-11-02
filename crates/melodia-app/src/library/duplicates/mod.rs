//! The duplicate finder, and the Not Duplicates list that hides a group the user keeps on purpose.
//!
//! A find reads the library in two passes so it never holds a row per track: a hashed key for
//! every track, then the full facts of only the candidates whose keys collide.

mod matcher;

use melodia_core::entities::duplicates::{DismissalSet, DuplicateCopy, DuplicateGroup};
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Every group in the library, dismissed ones included and flagged, in list order.
pub async fn find(db: &DbPool) -> Result<Vec<DuplicateGroup>, AppError> {
    let mut keys = Vec::new();
    queries::track::visit_duplicate_keys(db, |row| keys.push(matcher::SongKey::of(&row))).await?;
    let candidates = matcher::candidate_ids(keys);
    let copies = queries::track::duplicate_copies(db, &candidates).await?;
    Ok(matcher::group(copies))
}

/// The copies behind `ids` as they read now, for the finder's rows on screen.
pub async fn copies(db: &DbPool, ids: &[i64]) -> Result<Vec<DuplicateCopy>, AppError> {
    queries::track::duplicate_copies(db, ids).await
}

/// Not Duplicates on one group's copies, returning the set [`restore`] takes back.
pub async fn dismiss(db: &DbPool, track_ids: &[i64]) -> Result<DismissalSet, AppError> {
    queries::duplicate_dismissals::dismiss(db, track_ids).await
}

/// Count as Duplicates on a dismissed group.
pub async fn restore(db: &DbPool, set: DismissalSet) -> Result<(), AppError> {
    queries::duplicate_dismissals::restore(db, set).await
}
