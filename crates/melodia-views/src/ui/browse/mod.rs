//! The Browse page: the filesystem under the library folders, as a track list or a card grid.
//! Its models are installed once and mutated in place, never swapped, so Slint's bindings keep
//! their dependency tracks.
//!
//! A file on disk but not in the library gets a sparse row with `id == 0` and `enabled == false`,
//! which the row draws dimmed and inert. An empty `current-path` is the root, listing the library
//! folders as drillable rows, `browse_directory` refusing an empty path.
//!
//! Browse sorts in memory, mixing disk-only and library files where no `ORDER BY` reaches, and
//! keeps the sort on [`BrowseUi`] so a watcher's re-fetch preserves it. A fetch whose
//! `fetch_token` moved before its write lands drops the write.

mod breadcrumbs;
mod callbacks;
mod cards;
mod fetch;
mod models;
mod selection;

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::util::StringPool;
use crate::ui::view_ctx::ViewCtx;
use melodia_app::services::view_state;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::browse::{BrowseFile, BrowseFolder};
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::error::describe;
use melodia_ui::{
    AppWindow, Browse, BrowseCardGridRow as UiBrowseCardGridRow,
    BrowseFolderRow as UiBrowseFolderRow, TrackListRow as UiTrackListRow,
};

/// This page's `Nav.selected-index`, and the single definition of it.
pub const NAV_BROWSE: i32 = 1;

pub(super) use cards::{BrowseViewMode, mode_from_index, mode_index, rebuild_cards};
pub(super) use fetch::{apply_rows_favorite, apply_rows_rating, fetch_and_apply, resort_and_apply};
pub(super) use selection::{clear_selection, handle_select_row, select_all};

/// Wires every `Browse.*` callback and seeds the persisted path and presentation mode, here
/// because the seed needs the handle this builds.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<BrowseUi> {
    install_models(cx.app);
    install_selection_model(cx.app);
    let browse_ui = Arc::new(BrowseUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, cx.view_state, &browse_ui);
    seed_from_settings(cx.app, cx.state, &browse_ui, cx.view_state);
    browse_ui
}

/// The Browse page's handle, shared by its callbacks and its off-thread fetch.
pub struct BrowseUi {
    /// Empty at the root, and mirrored into `Browse.current-path` by every successful fetch.
    current_path: Mutex<String>,
    /// The paths drilled out of, for the back arrow.
    pub(super) history: Mutex<Vec<String>>,
    /// The last fetched files in display order, so callbacks read row data here rather than off
    /// the Slint model.
    pub(super) last_files: Mutex<Vec<BrowseFile>>,
    /// Kept for the card view, which draws folders and files in one grid and rebuilds it on a
    /// mode toggle with no fetch to take them from.
    pub(super) last_folders: Mutex<Vec<BrowseFolder>>,
    sort_field: Mutex<String>,
    sort_dir: Mutex<String>,
    /// The shared row tier; the cards draw from `grid_prewarm::tier()` instead.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Shadow of `Browse.view-mode` for the off-thread fetch, a bool because the index lives only
    /// in Slint.
    card_mode: AtomicBool,
    pub(super) fetch_token: AtomicU64,
    section: SectionState,
}

impl BrowseUi {
    fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            current_path: Mutex::new(String::new()),
            history: Mutex::new(Vec::new()),
            last_files: Mutex::new(Vec::new()),
            last_folders: Mutex::new(Vec::new()),
            sort_field: Mutex::new("title".to_owned()),
            sort_dir: Mutex::new("asc".to_owned()),
            cover_thumbs,
            card_mode: AtomicBool::new(false),
            fetch_token: AtomicU64::new(0),
            section: SectionState::new(),
        }
    }

    pub fn view_mode(&self) -> BrowseViewMode {
        if self.card_mode.load(Ordering::Relaxed) {
            BrowseViewMode::Card
        } else {
            BrowseViewMode::List
        }
    }

    pub fn set_view_mode(&self, mode: BrowseViewMode) {
        self.card_mode.store(mode == BrowseViewMode::Card, Ordering::Relaxed);
    }

    /// Decodes `paths` into the grid tier for a card view on screen. Blocking.
    ///
    /// Guarded here rather than at the fetch: Browse's boot seed fetches while the section is off
    /// screen, and a decode for it would stay resident for a page the user hasn't opened.
    pub fn warm_card_tier(&self, paths: &[PathBuf]) {
        if paths.is_empty() || !self.section_active() || self.view_mode() != BrowseViewMode::Card {
            return;
        }
        crate::ui::grid_prewarm::prewarm(paths);
    }

    /// [`Self::warm_card_tier`] over the cached listing, for the mode toggle.
    pub fn prewarm_card_covers(&self) {
        let unique = {
            let files = self.last_files.lock();
            cards::first_screenful_paths(&files)
        };
        self.warm_card_tier(&unique);
    }

    pub fn current_path(&self) -> String {
        self.current_path.lock().clone()
    }

    /// Pushes `leaving` onto the back-stack and lands on `going_to`.
    pub fn push_history(&self, leaving: String, going_to: String) {
        self.history.lock().push(leaving);
        *self.current_path.lock() = going_to;
    }

    pub fn pop_history(&self) -> Option<String> {
        let popped = self.history.lock().pop();
        if let Some(p) = popped.as_ref() {
            p.clone_into(&mut self.current_path.lock());
        }
        popped
    }

    /// Trims history back to `path` and lands on it. A `path` that isn't in the stack, a
    /// breadcrumb that stopped being an ancestor across a refresh, clears it instead.
    pub fn truncate_history_to(&self, path: &str) {
        let mut h = self.history.lock();
        if let Some(pos) = h.iter().position(|p| p == path) {
            h.truncate(pos);
        } else {
            h.clear();
        }
        path.clone_into(&mut self.current_path.lock());
    }

    /// Lands on `path` without touching history.
    pub fn set_path(&self, path: String) {
        *self.current_path.lock() = path;
    }

    pub fn sort_field(&self) -> String {
        self.sort_field.lock().clone()
    }

    pub fn sort_dir(&self) -> String {
        self.sort_dir.lock().clone()
    }

    pub fn set_sort(&self, field: String, dir: String) {
        *self.sort_field.lock() = field;
        *self.sort_dir.lock() = dir;
    }

    /// The in-library rows' ids in display order, what `play-row` queues.
    pub fn current_in_library_ids(&self) -> Vec<i64> {
        self.last_files.lock().iter().filter_map(|f| f.in_library.then_some(f.row.id)).collect()
    }

    /// Patches the cached rows, so a toggle needs no re-fetch.
    pub fn flip_favorites(&self, ids: &HashSet<i64>, fav: bool) {
        self.patch_cached_rows(ids, |row| row.is_favorite = fav);
    }

    /// [`Self::flip_favorites`]' star-rating twin.
    pub fn flip_ratings(&self, ids: &HashSet<i64>, rating: i32) {
        self.patch_cached_rows(ids, |row| row.rating = rating);
    }

    fn patch_cached_rows(&self, ids: &HashSet<i64>, edit: impl Fn(&mut RsTrackListRow)) {
        let mut files = self.last_files.lock();
        for file in files.iter_mut().filter(|f| ids.contains(&f.row.id)) {
            edit(&mut file.row);
        }
    }
}

/// Hands the global its empty `VecModel`s, which every later write downcasts back to.
fn install_models(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    let folders: Rc<VecModel<UiBrowseFolderRow>> = Rc::new(VecModel::default());
    let rows: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    let crumbs: Rc<VecModel<UiBreadcrumbRow>> = Rc::new(VecModel::default());
    let cards: Rc<VecModel<UiBrowseCardGridRow>> = Rc::new(VecModel::default());
    g.set_folders(ModelRc::from(folders));
    g.set_rows(ModelRc::from(rows));
    g.set_breadcrumbs(ModelRc::from(crumbs));
    g.set_card_rows(ModelRc::from(cards));
}

/// A persistent `Browse.selected-ids`, so a selection change is a `set_vec` rather than a fresh
/// `ModelRc` per click.
fn install_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Browse>().set_selected_ids(ModelRc::from(model));
}

/// Seeds the path and presentation mode from `views.json` and kicks the first fetch; with no
/// file, the root in list mode.
fn seed_from_settings(
    ui: &AppWindow,
    state: &AppState,
    browse_ui: &Arc<BrowseUi>,
    persisted: Option<&view_state::ViewStateData>,
) {
    let initial_path = persisted.and_then(|s| s.browse_path.clone()).unwrap_or_default();
    let persisted_mode = persisted.map_or(0, |s| s.browse_view_mode);

    browse_ui.set_path(initial_path.clone());
    let g = ui.global::<Browse>();
    g.set_current_path(SharedString::from(initial_path.as_str()));

    // Clamped against the Slint-declared count, the `tab-count` contract: a file
    // from a build with more presentations would select a branch mounting nothing.
    let mode_idx = crate::ui::tab_bar::clamp_tab(persisted_mode, g.get_view_mode_count());
    g.set_view_mode(mode_idx);
    browse_ui.set_view_mode(mode_from_index(&g, mode_idx));

    let weak = ui.as_weak();
    let state_clone = state.clone();
    let browse_clone = browse_ui.clone();
    state.runtime.spawn(async move {
        if let Err(e) = fetch_and_apply(&state_clone, &browse_clone, weak, initial_path).await {
            log::warn!("browse seed_from_settings fetch failed: {}", describe(&e));
        }
    });
}

use melodia_ui::BreadcrumbRow as UiBreadcrumbRow;

/// A `BrowseFile` as the shared `TrackListRow`, a disk-only one as the module doc's sparse row.
pub fn to_slint_browse_track_row(f: &BrowseFile, pool: &mut StringPool) -> UiTrackListRow {
    if f.in_library {
        crate::ui::tracks::to_slint_track_list_row(&f.row, pool)
    } else {
        UiTrackListRow {
            id: 0,
            title: SharedString::from(f.row.title.as_str()),
            enabled: false,
            ..UiTrackListRow::default()
        }
    }
}

/// [`to_slint_browse_track_row`] over a folder's files, sharing one pool across them.
pub fn to_slint_browse_track_rows(files: &[BrowseFile]) -> Vec<UiTrackListRow> {
    let mut pool = StringPool::default();
    files.iter().map(|f| to_slint_browse_track_row(f, &mut pool)).collect()
}

impl_section_state_helpers!(BrowseUi);

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<BrowseUi>();
};

#[cfg(test)]
#[path = "tests/browse_tests.rs"]
mod tests;
