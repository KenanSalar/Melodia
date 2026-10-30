//! Taking stations out of a kept tab, and the undo that puts them back.

use crate::tasks::artwork_sweep::ArtworkHold;
use melodia_core::entities::radio::{KeptList, StationCut};
use melodia_core::entities::undo::{UNDO_HOLD_CAP, UndoCapture};
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// What a removal from one kept tab took, and the logos kept from the sweep until the undo goes.
pub struct RemovedStations {
    from: KeptList,
    cuts: Vec<StationCut>,
    logos: ArtworkHold,
}

impl RemovedStations {
    pub fn list(&self) -> KeptList {
        self.from
    }

    /// A later removal from the same tab, for a run merged into one undo.
    pub fn absorb(&mut self, newer: Self) {
        self.cuts.extend(newer.cuts);
        self.logos.absorb(newer.logos);
    }

    pub fn count(&self) -> usize {
        self.cuts.len()
    }

    pub fn newest_name(&self) -> &str {
        self.cuts.last().map_or("", StationCut::name)
    }
}

/// Drops stations out of `from`, deleting each row neither tab lists any more, and holds what puts
/// them back. Past `UNDO_HOLD_CAP` stations it holds nothing and answers how many went.
///
/// Every station's logo is held: which rows go is only decided inside the transaction.
pub async fn remove_stations(
    db: &DbPool,
    ids: Vec<i64>,
    from: KeptList,
) -> Result<UndoCapture<RemovedStations>, AppError> {
    if ids.len() > UNDO_HOLD_CAP {
        let rows = remove_stations_for_good(db, &ids, from).await?;
        return Ok(if rows == 0 { UndoCapture::Nothing } else { UndoCapture::OverCap { rows } });
    }
    let logos = ArtworkHold::new(queries::radio_removal::station_artwork(db, &ids).await?);
    let cuts = queries::radio_removal::remove_stations(db, &ids, from).await?;
    Ok(if cuts.is_empty() {
        UndoCapture::Nothing
    } else {
        UndoCapture::Held(RemovedStations { from, cuts, logos })
    })
}

/// Drops stations out of `from` keeping nothing to undo it, and answers how many went.
pub async fn remove_stations_for_good(
    db: &DbPool,
    ids: &[i64],
    from: KeptList,
) -> Result<usize, AppError> {
    Ok(queries::radio_removal::remove_stations(db, ids, from).await?.len())
}

/// Undoes a removal. The logos stay held until the rows naming them are back.
pub async fn restore_stations(db: &DbPool, removed: RemovedStations) -> Result<(), AppError> {
    let RemovedStations { cuts, logos, .. } = removed;
    queries::radio_removal::restore_stations(db, &cuts).await?;
    drop(logos);
    Ok(())
}

/// Delete a row neither local tab would list, which nothing could reach. Browse's un-favorite owes
/// this, [`super::set_directory_favorite`] not deciding.
pub async fn delete_if_unlisted(db: &DbPool, id: i64) -> Result<(), AppError> {
    queries::radio_removal::delete_if_unlisted(db, id).await
}
