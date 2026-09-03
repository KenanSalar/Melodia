//! The Browse page: the filesystem under the library folders, as a track list
//! or a card grid.
//!
//! Its models are installed once at startup and mutated **in place** on every
//! navigation — no `ModelRc` swaps after install, so Slint's reactive bindings
//! keep their dependency tracks.
//!
//! `rows` renders through the shared `TrackList`. In-library files carry their
//! real `TrackListRow`; a file on disk that isn't in the DB carries a synthesized
//! sparse row with `id == 0` and `enabled == false`, so the row item draws it
//! dimmed and swallows every interaction.
//!
//! `current-path == ""` is the **root** state: rather than call
//! `browse_directory`, which rejects an empty path, Rust lists the library
//! folders as drillable rows — and flips `has_library_folders` false when there
//! are none, so the view paints its CTA.
//!
//! Browse sorts **in memory**, mixing disk-only and DB files, so unlike the
//! Tracks view it can't push an `ORDER BY`. The sort state lives on `BrowseUi`,
//! so a watcher-driven re-fetch preserves the user's chosen order.
//!
//! Every [`fetch_and_apply`] bumps `fetch_token` and captures the post-bump
//! value; a late fetch whose token has moved by the time its closure runs drops
//! its UI write rather than overwriting a newer result.

mod breadcrumbs;
mod callbacks;
mod cards;
mod fetch;
mod models;
mod selection;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::browse::{BrowseFile, BrowseFolder};
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::services::view_state;
use crate::state::AppState;
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AppWindow, Browse, BrowseCardGridRow as UiBrowseCardGridRow,
    BrowseFolderRow as UiBrowseFolderRow, TrackListRow as UiTrackListRow,
};

// `boot::ui_setup` retunes the cover cap once the window is live.
pub use cards::tune_cache_for_display;

/// Browse's `Nav.selected-index` — see [`crate::ui::favorites::NAV_FAVORITES`].
pub const NAV_BROWSE: i32 = 1;

// `pub(super)` is `pub(in crate::ui)` here, which is exactly the reach these
// need: this slice's own `callbacks.rs`, plus the cross-slice `apply_row_*`
// mirrors in `callbacks::now_playing`.
pub(super) use cards::{BrowseViewMode, mode_from_index, mode_index, rebuild_cards};
pub(super) use fetch::{apply_row_favorite, apply_row_rating, fetch_and_apply, resort_and_apply};
pub(super) use selection::{clear_selection, handle_select_row};

/// Install the Browse models, build the handle, wire every `Browse.*` callback,
/// and seed the persisted path + presentation mode.
///
/// The seed folds in here, being Browse-local and needing only the handle this
/// call just built — which is also what lets it take `views.json` as an argument
/// rather than re-reading a file `install_views` already parsed.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<BrowseUi> {
    install_models(cx.app);
    install_selection_model(cx.app);
    let browse_ui = Arc::new(BrowseUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, cx.view_state, &browse_ui);
    crate::ui::cover_generation::notify_on_decode(
        &browse_ui.grid_covers,
        cx.app,
        cards::repaint_covers,
    );
    seed_from_settings(cx.app, cx.state, &browse_ui, cx.view_state);
    browse_ui
}

/// Rust-side state for the Browse view. Shared between the UI callbacks
/// (`callbacks::wire`) and the async fetcher.
pub struct BrowseUi {
    /// Canonical current location. `""` ⇒ root (library folder list).
    /// Mirrored into `Browse.current-path` on every successful fetch.
    current_path: Mutex<String>,
    /// Back-stack of paths the user has drilled out of. `go_back` pops
    /// and refetches; `open_folder` pushes the current path before
    /// switching.
    pub(super) history: Mutex<Vec<String>>,
    /// Last fetched files, already in display order — `play-row`, the selection
    /// helpers and the favourite toggle recover full row data from here rather
    /// than round-tripping the Slint model.
    pub(super) last_files: Mutex<Vec<BrowseFile>>,
    /// Cached beside `last_files` because the card view draws both in one grid,
    /// and a mode toggle rebuilds it with no fetch to take them from.
    pub(super) last_folders: Mutex<Vec<BrowseFolder>>,
    /// In-memory sort state — Browse mixes disk-only and DB files, so it can't
    /// push an `ORDER BY`.
    sort_field: Mutex<String>,
    sort_dir: Mutex<String>,
    /// The shared row tier.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The card view's own tier — private, so releasing it can't yank the row
    /// thumbnails the shared one is still serving. Released on section leave and
    /// on going back to the list.
    pub(super) grid_covers: Arc<CoverThumbs>,
    /// Synchronous shadow of `Browse.view-mode`, read by the fetch off a tokio
    /// worker where the global is out of reach. A bool because the index itself
    /// lives only in Slint.
    card_mode: AtomicBool,
    /// Stale-fetch guard — see the module docs.
    pub(super) fetch_token: AtomicU64,
    /// Visibility and staleness. Browse releases nothing on leave, so `dirty` is
    /// set only by the `library_changed` subscriber while hidden, and consumed
    /// on re-enter to re-fetch the current directory once.
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
            grid_covers: Arc::new(CoverThumbs::with_config(
                crate::ui::grid_prewarm::GRID_COVER_FALLBACK,
                cards::DEFAULT_GRID_COVER_CAP,
            )),
            card_mode: AtomicBool::new(false),
            fetch_token: AtomicU64::new(0),
            section: SectionState::new(),
        }
    }

    /// Which presentation is mounted.
    pub fn view_mode(&self) -> BrowseViewMode {
        if self.card_mode.load(Ordering::Relaxed) {
            BrowseViewMode::Card
        } else {
            BrowseViewMode::List
        }
    }

    /// Mirror `Browse.view-mode` into the synchronous shadow.
    pub fn set_view_mode(&self, mode: BrowseViewMode) {
        self.card_mode.store(mode == BrowseViewMode::Card, Ordering::Relaxed);
    }

    /// Resolve one card's cover, decoding only once the tier is known warm — a
    /// `generation` of `0` means just toggled or just re-entered, so answer from
    /// the cache and let the card paint its placeholder rather than putting a
    /// decode per visible card on the UI thread.
    pub fn grid_cover(&self, artwork_path: &str, generation: i32) -> slint::Image {
        crate::ui::grid_prewarm::grid_cover(&self.grid_covers, artwork_path, generation)
    }

    /// Decode `paths` into the card tier. Blocking — call from `spawn_blocking`.
    ///
    /// Returns whether the tier is warm **and still holds what was decoded** —
    /// the answer a caller's `covers-generation` bump is gated on. `false` when
    /// a leave or a toggle back to the list landed inside the decode, where the
    /// buffers are handed straight back: announcing a tier the leave already
    /// released puts the next cards on the decoding path. The two warm sites
    /// differ only in where the paths come from, so the re-check lives here.
    ///
    /// An empty `paths` is still a warm tier — a directory of nothing but
    /// subfolders has no cover to wait for.
    pub fn warm_card_tier(&self, paths: &[PathBuf]) -> bool {
        if !paths.is_empty() {
            self.grid_covers.prewarm(paths);
        }
        // Re-checked *after* the decode — before it, the leave hasn't happened yet.
        if self.section_active() && self.view_mode() == BrowseViewMode::Card {
            return true;
        }
        self.release_grid_covers();
        false
    }

    /// [`Self::warm_card_tier`] over the cached listing — the mode toggle's path,
    /// which has no fetch to take a fresh file list from.
    pub fn prewarm_card_covers(&self) -> bool {
        if self.view_mode() != BrowseViewMode::Card {
            return false;
        }
        let unique = {
            let files = self.last_files.lock();
            cards::first_screenful_paths(&files)
        };
        self.warm_card_tier(&unique)
    }

    /// Drop every card cover. Paired with a `covers-generation` rewind by the
    /// caller, so `0` keeps meaning "this tier is cold" rather than "first
    /// toggle of the session".
    pub fn release_grid_covers(&self) {
        self.grid_covers.clear();
        crate::services::allocator::trim();
    }

    pub fn current_path(&self) -> String {
        self.current_path.lock().clone()
    }

    /// Push `leaving` onto the back-stack and land on `going_to`.
    pub fn push_history(&self, leaving: String, going_to: String) {
        self.history.lock().push(leaving);
        *self.current_path.lock() = going_to;
    }

    /// Pop the most recent entry off the back-stack.
    pub fn pop_history(&self) -> Option<String> {
        let popped = self.history.lock().pop();
        if let Some(p) = popped.as_ref() {
            p.clone_into(&mut self.current_path.lock());
        }
        popped
    }

    /// Trim history back to `path` and land on it. A `path` that isn't in the
    /// stack — a breadcrumb that stopped being an ancestor across a refresh —
    /// clears it instead.
    pub fn truncate_history_to(&self, path: &str) {
        let mut h = self.history.lock();
        if let Some(pos) = h.iter().position(|p| p == path) {
            h.truncate(pos);
        } else {
            h.clear();
        }
        path.clone_into(&mut self.current_path.lock());
    }

    /// Replace `current_path` without touching history — the refresh path and
    /// the initial seed.
    pub fn set_path(&self, path: String) {
        *self.current_path.lock() = path;
    }

    pub fn sort_field(&self) -> String {
        self.sort_field.lock().clone()
    }

    pub fn sort_dir(&self) -> String {
        self.sort_dir.lock().clone()
    }

    /// Store the sort, for the next `fetch_and_apply` / `resort_and_apply`.
    pub fn set_sort(&self, field: String, dir: String) {
        *self.sort_field.lock() = field;
        *self.sort_dir.lock() = dir;
    }

    /// Ids of the in-library `last_files` rows, in display order — what
    /// `play-row` hands to `player_play_tracks`.
    pub fn current_in_library_ids(&self) -> Vec<i64> {
        self.last_files.lock().iter().filter_map(|f| f.in_library.then_some(f.row.id)).collect()
    }

    /// Flip `is_favorite` on the cached row, so a single-row toggle needs no
    /// re-fetch.
    pub fn flip_favorite(&self, id: i64, fav: bool) {
        let mut files = self.last_files.lock();
        if let Some(f) = files.iter_mut().find(|f| f.row.id == id) {
            f.row.is_favorite = fav;
        }
    }

    /// [`Self::flip_favorite`]'s star-rating twin.
    pub fn flip_rating(&self, id: i64, rating: i32) {
        let mut files = self.last_files.lock();
        if let Some(f) = files.iter_mut().find(|f| f.row.id == id) {
            f.row.rating = rating;
        }
    }
}

/// Hand the `Browse` global its four empty `VecModel`s. Later updates find them
/// by downcasting back and mutating in place.
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

/// A persistent model for `Browse.selected-ids`, so a selection change is a
/// `set_vec` rather than a fresh `ModelRc` per click. Mirrors
/// `tracks::install_selection_model`.
fn install_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Browse>().set_selected_ids(ModelRc::from(model));
}

/// Seed the path and presentation mode from `views.json` and kick the initial
/// fetch. `None` — a fresh install, or a file that wouldn't read — lands at the
/// root in list mode. Takes the boot read rather than repeating it.
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
            log::warn!("browse seed_from_settings fetch failed: {e}");
        }
    });
}

use crate::BreadcrumbRow as UiBreadcrumbRow;

/// A `BrowseFile` as the shared `TrackListRow`. An in-library file reuses the
/// Tracks converter; a disk-only one becomes the sparse, dimmed,
/// non-interactive row the module docs describe.
pub fn to_slint_browse_track_row(f: &BrowseFile) -> UiTrackListRow {
    if f.in_library {
        let mut row = crate::ui::tracks::to_slint_track_list_row(&f.row);
        row.enabled = true;
        row
    } else {
        UiTrackListRow {
            id: 0,
            title: SharedString::from(f.row.title.as_str()),
            artist: SharedString::from(""),
            album: SharedString::from(""),
            genre: SharedString::from(""),
            year: 0,
            track_number: 0,
            duration_ms: 0,
            is_favorite: false,
            rating: 0,
            artwork_path: SharedString::from(""),
            display_duration: SharedString::from(""),
            selected: false,
            enabled: false,
            album_id: 0,
            artist_id: 0,
            genre_id: 0,
        }
    }
}

impl_section_state_helpers!(BrowseUi);

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<BrowseUi>();
};
