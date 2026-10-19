//! [`PagedRows`] over the queue sheet's rows, and the row shape Up Next draws too.
//!
//! The queue holds ids, as long as the library at its longest, so the sheet fetches its rows by id
//! like any other list that can hold the whole library. They come through the track-list
//! projection, which carries the album, artist and genre ids the row menu's "Go to" entries need.

use std::sync::Arc;

use super::track_rows::fetch_by_id;
use super::{Keyset, PagedRow, PagedRows, ReadStamp, RowRequest, window};
use crate::ui::tracks::format_duration_ms;
use crate::ui::util::{StringPool, clamp_i64_to_i32};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::error::AppResult;
use melodia_ui::QueueRow;

pub type QueueRows = PagedRows<QueueRow>;

impl PagedRow for QueueRow {
    // Blank but live: a row clicked before its fetch lands still skips to, picks and removes by
    // position and id.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }

    fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
    }
}

/// Fetches the rows the queue sheet asks for.
pub fn fetcher(state: &AppState) -> impl Fn(RowRequest<QueueRow>) + 'static {
    fetch_by_id(state, convert)
}

/// The queue's `ids` with the rows of the [`window`] around `at`, so the sheet opens on text where
/// it seats the current track rather than on placeholders.
pub async fn seed(state: &AppState, ids: Arc<[i64]>, at: usize) -> AppResult<Keyset<QueueRow>> {
    let read = ReadStamp::now();
    let seed =
        convert(&library::keyset::rows_by_ids(&state.db, &ids[window(ids.len(), at)]).await?);
    Ok(Keyset { ids, seed, read })
}

/// The rows of one fetch, sharing a pool so an album's tracks share their strings.
pub fn convert(rows: &[RsTrackListRow]) -> Vec<(i64, QueueRow)> {
    let mut pool = StringPool::default();
    rows.iter().map(|row| (row.id, to_queue_row(row, &mut pool))).collect()
}

/// One track as a queue row, unselected. Carries no cover: the sheet and Up Next each resolve
/// their own through their `request-cover`, which is what lets them share this while reading
/// different cover tiers.
pub fn to_queue_row(row: &RsTrackListRow, pool: &mut StringPool) -> QueueRow {
    QueueRow {
        id: clamp_i64_to_i32(row.id),
        title: row.title.as_str().into(),
        artist: pool.intern(row.artist.as_deref().unwrap_or_default()),
        artwork_path: pool.intern(row.artwork_path.as_deref().unwrap_or_default()),
        display_duration: pool.intern(&format_duration_ms(row.duration_ms.max(0))),
        selected: false,
        is_favorite: row.is_favorite,
        rating: row.rating,
        album_id: row.album_id.map_or(0, clamp_i64_to_i32),
        artist_id: row.artist_id.map_or(0, clamp_i64_to_i32),
        genre_id: row.genre_id.map_or(0, clamp_i64_to_i32),
    }
}

#[cfg(test)]
#[path = "tests/queue_rows_tests.rs"]
mod tests;
