//! `views.json`: per-view UI state, kept apart from the preferences in `settings.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::services::settings::{ColumnWidths, ViewSort};
use melodia_core::config::Paths;
use melodia_core::error::AppResult;

/// Per-view UI state. A missing key or file reads as a first launch.
///
/// The maps are `BTreeMap`s so the same state always saves as the same file, where a `HashMap`
/// read back iterates in an order of its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewStateData {
    /// Keyed by view id (`melodia-views`' `track_list_view::view_id`), like the maps below.
    pub view_columns: BTreeMap<String, Vec<String>>,
    /// Whether a track list shows each row mark, keyed like `view_columns` and then by mark. Not
    /// more ids in `view_columns`: every exit saves that list, so an id added later would read as
    /// unticked on every install that has run once, where a mark missing here takes the default.
    pub view_marks: BTreeMap<String, BTreeMap<String, bool>>,
    pub view_column_widths: BTreeMap<String, ColumnWidths>,
    pub view_sort: BTreeMap<String, ViewSort>,
    /// `None` lands at the library root.
    pub browse_path: Option<String>,
    /// Clamped on read through `ui::tab_bar::clamp_tab`, as every tab index below is, so a value
    /// from a build with more modes can't mount nothing.
    pub browse_view_mode: i32,
    /// `Nav.selected-index`. An older file can hold the retired 4–7, which
    /// `ui::my_library::fold_retired_nav_index` folds onto 3.
    pub last_nav_index: i32,
    /// An absent entry lands that view on its root.
    pub last_detail_ids: BTreeMap<String, i64>,
    /// Artist Detail's Albums toggle, handing the scroller's space to the track list.
    pub artist_albums_collapsed: bool,
    /// The Settings category, by its key, so a category inserted mid-list renumbers nothing saved.
    /// Unknown or empty opens on Library. An older file's ints, `settings_tab` and
    /// `settings_category`, are left unread rather than folded, their numbers overlapping.
    ///
    /// **A persisted view flag is an int, a string or a map, never a bool**: this struct sits at
    /// clippy's `struct_excessive_bools` cap.
    pub settings_category_key: String,
    /// The four page tabs, each clamped against its own global's count.
    pub favorites_tab: i32,
    pub recently_played_tab: i32,
    pub my_library_tab: i32,
    pub radio_tab: i32,
}

impl Default for ViewStateData {
    fn default() -> Self {
        Self {
            view_columns: BTreeMap::new(),
            view_marks: BTreeMap::new(),
            view_column_widths: BTreeMap::new(),
            view_sort: BTreeMap::new(),
            browse_path: None,
            browse_view_mode: 0,
            last_nav_index: default_last_nav_index(),
            last_detail_ids: BTreeMap::new(),
            artist_albums_collapsed: false,
            settings_category_key: String::new(),
            favorites_tab: 0,
            recently_played_tab: 0,
            my_library_tab: 0,
            radio_tab: 0,
        }
    }
}

/// `nav.slint`'s own `Nav.selected-index` default.
fn default_last_nav_index() -> i32 {
    3
}

/// The highest index `nav.slint` routes, shared by the write's clamp in
/// `library::settings::set_last_nav_index` and the read's guard in
/// `boot::ui_setup::views::install_views`, which must agree or a boot silently lands elsewhere.
pub const MAX_NAV_INDEX: i32 = 10;

pub fn read_view_state(paths: &Paths) -> AppResult<ViewStateData> {
    melodia_core::utils::atomic_file::load_json_or_default_sync(&paths.view_state_path)
}

pub fn write_view_state(paths: &Paths, view_state: &ViewStateData) -> AppResult<()> {
    melodia_core::utils::atomic_file::write_json_sync(&paths.view_state_path, view_state)
}

/// `settings`' `MUTATE_LOCK`, for `views.json`.
static MUTATE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Reads, mutates and writes `views.json` under `MUTATE_LOCK`.
pub fn mutate_view_state<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut ViewStateData),
{
    mutate_view_state_with(paths, mutate)
}

/// [`mutate_view_state`], handing back the closure's value.
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
