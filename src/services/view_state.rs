//! `views.json` persistence: the serde model for per-view UI state, plus its
//! load / save / atomic read-mutate-write I/O.
//!
//! Separate from `settings.json` so genuine app preferences don't share a file
//! with transient per-view layout state — a purpose-scoped sibling like
//! `queue.json` and `search_history.json`. The shared boundary types stay
//! defined in [`super::settings`] and are imported here.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::Paths;
use crate::error::{AppError, AppResult};
use crate::services::settings::{ColumnWidths, ViewSort};

/// Per-view UI state persisted between launches. Every field is
/// `#[serde(default)]` through the struct attribute, so a missing key — or a
/// missing file — falls back to first-launch behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewStateData {
    /// Keyed by view-id (`src/ui/track_list_view.rs::view_id`); one map each
    /// covers every list or grid view.
    pub view_columns: HashMap<String, Vec<String>>,
    pub view_column_widths: HashMap<String, ColumnWidths>,
    pub view_sort: HashMap<String, ViewSort>,
    /// Browse's folder at last shutdown. `None` lands at the library root.
    pub browse_path: Option<String>,
    /// Browse's presentation. Clamped on read through `ui::tab_bar::clamp_tab`,
    /// the guard every tab index below takes, so a value from a build with more
    /// modes can't select a branch that mounts nothing.
    pub browse_view_mode: i32,
    /// Sidebar tab at last shutdown, hydrated into `Nav.selected-index`. A file
    /// written before the five library views became one page can hold 4–7;
    /// `ui::my_library::fold_retired_nav_index` maps those onto 3 on the way in.
    pub last_nav_index: i32,
    /// Detail ids open at last shutdown, keyed by view-id. An absent entry lands
    /// that view on its root.
    pub last_detail_ids: HashMap<String, i64>,
    /// Artist Detail's Albums header toggle. `true` hides the scroller so the
    /// all-tracks list claims that space.
    pub artist_albums_collapsed: bool,
    /// The five page tabs at last shutdown, each clamped against its own
    /// global's `tab-count`.
    ///
    /// An index rather than a set of bools, and that is the rule rather than a
    /// preference: `favorites_tab` replaced two `*_collapsed` flags that sat
    /// with `artist_albums_collapsed` exactly at clippy's
    /// `struct_excessive_bools` cap. **A persisted view flag is an int, a string
    /// or a map** — never a bool, whatever the current count.
    pub settings_tab: i32,
    pub favorites_tab: i32,
    pub recently_played_tab: i32,
    pub my_library_tab: i32,
    pub radio_tab: i32,
}

impl Default for ViewStateData {
    fn default() -> Self {
        Self {
            view_columns: HashMap::new(),
            view_column_widths: HashMap::new(),
            view_sort: HashMap::new(),
            browse_path: None,
            browse_view_mode: 0,
            last_nav_index: default_last_nav_index(),
            last_detail_ids: HashMap::new(),
            artist_albums_collapsed: false,
            settings_tab: 0,
            favorites_tab: 0,
            recently_played_tab: 0,
            my_library_tab: 0,
            radio_tab: 0,
        }
    }
}

/// Matches `nav.slint`'s own `Nav.selected-index` default, so a fresh install
/// lands where the Slint side already points.
fn default_last_nav_index() -> i32 {
    3
}

/// The highest index `nav.slint` routes. Bounds [`ViewStateData::last_nav_index`]
/// at both ends of its round trip — the clamp in
/// `library::settings::set_last_nav_index` and the guard in
/// `boot::ui_setup::install_views` — and the two have to agree, a section written
/// past the write's clamp and one dropped by the read's guard both landing the
/// next boot somewhere else with nothing to say why.
pub const MAX_NAV_INDEX: i32 = 10;

pub fn read_view_state(paths: &Paths) -> AppResult<ViewStateData> {
    crate::services::load_json_or_default_sync(&paths.view_state_path)
}

pub fn write_view_state(paths: &Paths, view_state: &ViewStateData) -> AppResult<()> {
    crate::services::write_json_atomic_sync(&paths.view_state_path, view_state)
        .map_err(|e| AppError::Settings(format!("Failed to write view state: {e}")))
}

/// Held across the whole read→mutate→write window, so a burst of UI events can't
/// interleave their read snapshots and lose updates. `settings`'s `MUTATE_LOCK`
/// shape; the plain read and write stay lock-free, a single-shot read and a
/// full-replacement write being race-free against each other.
static MUTATE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Atomically read → mutate → write `views.json`, serialized against every other
/// caller so concurrent partial updates merge without tearing each other's reads.
pub fn mutate_view_state<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut ViewStateData),
{
    mutate_view_state_with(paths, mutate)
}

/// [`mutate_view_state`] returning a value the caller wants to keep. Mirrors
/// `services::settings::mutate_settings_with`.
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
