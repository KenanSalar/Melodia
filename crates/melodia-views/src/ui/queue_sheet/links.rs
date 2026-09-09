//! The album/artist/genre linkage behind the queue row menu's "Go to …"
//! entries, and the cache that keeps a re-open from re-asking for it.
//!
//! `QueueRow` is built from a `TrackSummary`, which carries playback identity
//! and no foreign keys — widening it would put three columns into every
//! published view model and into `queue.json`. So the ids are fetched beside
//! the rows and patched in, the shape [`super::rows::apply_row_favorite`]
//! already uses for a favourite toggle.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use slint::{ComponentHandle, Model, VecModel, Weak};

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackLinks;
use melodia_ui::{AppWindow, Queue, QueueRow};

/// Track id → its FK trio, for every queue row resolved so far this open.
/// Cleared beside the cover tier when the sheet tears down, so a closed sheet
/// holds nothing.
pub(super) type LinkCache = Arc<Mutex<HashMap<i64, TrackLinks>>>;

/// The FK trio as the Slint row spells it: 0 for absent, so the menu's
/// `album-id != 0` gate reads a missing linkage and a not-yet-fetched one the
/// same way. That is the honest reading — until the fetch lands, we don't know.
#[derive(Clone, Copy, Default)]
pub(super) struct RowLinks {
    pub album: i32,
    pub artist: i32,
    pub genre: i32,
}

impl RowLinks {
    /// Write the trio onto a row, and say whether anything moved — which is what
    /// lets the patch skip a `set_row_data` that would invalidate the
    /// `ListView`'s delegate cache for nothing.
    pub(super) fn stamp(self, row: &mut QueueRow) -> bool {
        if row.album_id == self.album && row.artist_id == self.artist && row.genre_id == self.genre
        {
            return false;
        }
        row.album_id = self.album;
        row.artist_id = self.artist;
        row.genre_id = self.genre;
        true
    }
}

impl From<TrackLinks> for RowLinks {
    fn from(l: TrackLinks) -> Self {
        Self {
            album: to_row_id(l.album_id),
            artist: to_row_id(l.artist_id),
            genre: to_row_id(l.genre_id),
        }
    }
}

fn to_row_id(id: Option<i64>) -> i32 {
    id.and_then(|v| i32::try_from(v).ok()).unwrap_or(0)
}

/// Resolve the ids the cache is missing and patch them into the live rows.
///
/// A no-op when nothing is missing, which is every rebuild after the first:
/// reorders, removes and re-opens all reuse what the previous pass resolved.
pub(super) fn fetch_missing(
    state: &AppState,
    weak: &Weak<AppWindow>,
    cache: &LinkCache,
    is_open: &Arc<AtomicBool>,
    missing: Vec<i64>,
) {
    if missing.is_empty() {
        return;
    }
    let state = state.clone();
    let weak = weak.clone();
    let cache = cache.clone();
    let is_open = is_open.clone();
    state.runtime.clone().spawn(async move {
        let links = match library::tracks::get_track_links(&state, &missing).await {
            Ok(links) => links,
            Err(e) => {
                log::warn!("queue sheet: track links for {} rows: {e}", missing.len());
                return;
            }
        };
        {
            let mut guard = cache.lock();
            guard.extend(links.iter().map(|l| (l.id, *l)));
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            // The teardown 350 ms after a close empties the model; landing past
            // it would refill rows nothing is looking at, against a cover tier
            // that has already been dropped.
            if !is_open.load(Ordering::Relaxed) {
                return;
            }
            patch_rows(&ui, &links);
        });
    });
}

/// Stamp the resolved trio onto every visible row it belongs to. Keyed by
/// track id rather than by position, so a queue reordered while the fetch was
/// in flight still lands on the right rows — and a track sitting in the queue
/// twice gets both.
fn patch_rows(ui: &AppWindow, links: &[TrackLinks]) {
    let rows = ui.global::<Queue>().get_rows();
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<QueueRow>>() else {
        return;
    };
    let by_id: HashMap<i32, RowLinks> = links
        .iter()
        .filter_map(|l| i32::try_from(l.id).ok().map(|id| (id, RowLinks::from(*l))))
        .collect();

    for i in 0..vm.row_count() {
        let Some(mut row) = vm.row_data(i) else {
            continue;
        };
        let Some(resolved) = by_id.get(&row.id) else {
            continue;
        };
        if resolved.stamp(&mut row) {
            vm.set_row_data(i, row);
        }
    }
}
