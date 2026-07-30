//! `views.json` persistence: the serde data model for per-view UI state
//! plus its load / save / atomic read-mutate-write I/O.
//!
//! Kept separate from `settings.json` (see [`super::settings`]) so genuine
//! app/user preferences (theme, locale, playback, window geometry) don't
//! share a file with transient per-view layout state — column widths,
//! column visibility, sort, browse path, the active nav tab, open detail
//! ids, and section-collapse toggles. Both files live side-by-side in the
//! data dir; this one mirrors `queue.json` / `search_history.json` as a
//! purpose-scoped sibling.
//!
//! The shared boundary types ([`ColumnWidths`], [`ViewSort`]) stay defined
//! in [`super::settings`] and are imported here — they're re-exported as
//! `services::settings::*` and used by callbacks / UI code that predates
//! this split.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::Paths;
use crate::error::{AppError, AppResult};
use crate::services::settings::{ColumnWidths, ViewSort};

/// Per-view UI state persisted to `views.json` between launches. Every
/// field is `#[serde(default)]` via the struct attribute, so a missing key
/// (or a missing file) falls back to [`ViewStateData::default`] — matching
/// first-launch behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewStateData {
    /// Per-view visible column ids in display order, keyed by view-id
    /// (see `src/ui/track_list_view.rs::view_id`).
    pub view_columns: HashMap<String, Vec<String>>,
    /// Per-view track-list column widths, keyed by view-id.
    pub view_column_widths: HashMap<String, ColumnWidths>,
    /// Per-view sort (field + direction), keyed by view-id. One map covers
    /// every sortable list/grid view.
    pub view_sort: HashMap<String, ViewSort>,
    /// Browse view's current folder path, so the next launch reopens at
    /// the same location. `None` ⇒ land at the library-folder root.
    pub browse_path: Option<String>,
    /// Sidebar tab active at last shutdown. Hydrated into
    /// `Nav.selected-index` at startup. Indices follow the `Nav` global
    /// comment (0=search, 1=browse, 2=favorites, 3=tracks, 4=albums,
    /// 5=artists, 6=genres, 7=playlists, 8=recently-played, 9=settings).
    pub last_nav_index: i32,
    /// Detail-view ids open at last shutdown, keyed by view-id string.
    /// Entry present ⇒ that view's detail page is hydrated at startup;
    /// entry absent ⇒ that view lands on its root (grid / list).
    pub last_detail_ids: HashMap<String, i64>,
    /// Artist Detail "Albums" sub-section header toggle. `true` hides the
    /// horizontal album scroller so the all-tracks list claims that space.
    pub artist_albums_collapsed: bool,
    /// Favorites view "Favorite Artists" sub-section header toggle.
    pub favorites_artists_collapsed: bool,
    /// Favorites view "Most Played" sub-section header toggle.
    pub favorites_most_played_collapsed: bool,
}

impl Default for ViewStateData {
    fn default() -> Self {
        Self {
            view_columns: HashMap::new(),
            view_column_widths: HashMap::new(),
            view_sort: HashMap::new(),
            browse_path: None,
            last_nav_index: default_last_nav_index(),
            last_detail_ids: HashMap::new(),
            artist_albums_collapsed: false,
            favorites_artists_collapsed: false,
            favorites_most_played_collapsed: false,
        }
    }
}

/// Matches the `Nav.selected-index` default in `melodia-ui/ui/globals/nav.slint`
/// (3 = Tracks). Fresh installs without a `views.json` land on the
/// Tracks view.
fn default_last_nav_index() -> i32 {
    3
}

pub fn read_view_state(paths: &Paths) -> AppResult<ViewStateData> {
    crate::services::load_json_or_default_sync(&paths.view_state_path)
}

pub fn write_view_state(paths: &Paths, view_state: &ViewStateData) -> AppResult<()> {
    crate::services::write_json_atomic_sync(&paths.view_state_path, view_state)
        .map_err(|e| AppError::Settings(format!("Failed to write view state: {e}")))
}

/// Process-wide lock around `views.json` mutation. Held by
/// `mutate_view_state` for the entire read→mutate→write window so a burst
/// of UI events (e.g. rapid column toggles across views) can't interleave
/// their read snapshots and lose updates. Mirrors `settings`'s
/// `MUTATE_LOCK`; plain `read_view_state` / `write_view_state` stay
/// lock-free — single-shot reads and full-replacement writes are
/// inherently race-free against each other.
static MUTATE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Atomically read → mutate → write `views.json`. Serializes against every
/// other `mutate_view_state` caller so concurrent partial-update callsites
/// (`update_view_columns`, `set_view_sort`, …) merge cleanly without
/// tearing each other's reads.
pub fn mutate_view_state<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut ViewStateData),
{
    mutate_view_state_with(paths, mutate)
}

/// Read → inspect/mutate → write `views.json` under the same `MUTATE_LOCK`
/// as `mutate_view_state`, returning a value the caller wants to retain.
/// Mirrors `services::settings::mutate_settings_with`.
pub fn mutate_view_state_with<F, R>(paths: &Paths, mutate: F) -> AppResult<R>
where
    F: FnOnce(&mut ViewStateData) -> R,
{
    let _guard = MUTATE_LOCK.lock();
    let mut view_state = read_view_state(paths)?;
    let out = mutate(&mut view_state);
    write_view_state(paths, &view_state)?;
    Ok(out)
}

#[cfg(test)]
#[path = "tests/view_state_tests.rs"]
mod tests;
