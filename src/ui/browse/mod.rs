//! Browse-view glue between Rust and Slint.
//!
//! The Slint `Browse` global owns three `Rc<VecModel<_>>` lists
//! (`folders`, `rows`, `breadcrumbs`) installed once at startup via
//! `install_models`, plus a persistent `VecModel<i32>` for the
//! selection (`install_selection_model`). `fetch_and_apply`
//! mutates the existing `VecModel`s in place on every navigation — no
//! `ModelRc` swaps after install, so Slint's reactive bindings keep their
//! dependency tracks.
//!
//! `rows` is rendered by the shared `TrackList` component (the same one
//! the Tracks view uses). In-library files carry their real DB
//! `TrackListRow`; disk-only files (present on disk inside a library
//! folder but not yet in the DB) carry a synthesized sparse row with
//! `id == 0` / `enabled == false`, so the shared row item renders them
//! dimmed and swallows every interaction.
//!
//! `current-path == ""` is the special **root** state: instead of
//! calling `library::browse::browse_directory` (which would reject an
//! empty path), Rust fetches the library folder list via
//! `library::settings::get_folders` and renders them as drillable folder
//! rows. `has_library_folders` flips false when that list is empty so
//! the view paints a "No music folders configured" CTA.
//!
//! Browse sorts **in-memory** in Rust: it mixes disk-only and DB files,
//! so unlike the Tracks view it can't re-query the DB with an `ORDER BY`.
//! `sort_browse_files` reorders the cached `last_files` Vec; the sort
//! field/direction live on `BrowseUi` so a watcher-driven re-fetch
//! preserves the user's chosen order.
//!
//! Cross-thread layout:
//! * `BrowseUi` is `Send + Sync` — `Arc<BrowseUi>` cloned into callbacks
//!   and tokio tasks.
//! * Slint properties/models can only be touched from the UI thread; we
//!   hop back via `Weak<AppWindow>::upgrade_in_event_loop`.
//!
//! Stale-fetch guard: every call to [`fetch_and_apply`] bumps
//! `fetch_token` and captures its post-bump value. If a later fetch
//! has incremented the token by the time the UI-thread closure runs,
//! the late fetch drops its UI write so it doesn't overwrite a newer
//! result.

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
use crate::media::cover_thumbs::CoverThumbs;
use crate::services::view_state;
use crate::ui::section_state::SectionState;
use crate::ui::view_ctx::ViewCtx;
use crate::state::AppState;
use crate::{
    AppWindow, Browse, BrowseCardGridRow as UiBrowseCardGridRow,
    BrowseFolderRow as UiBrowseFolderRow, TrackListRow as UiTrackListRow,
};

// `boot::ui_setup` retunes the cover cap once the window is live.
pub use cards::tune_cache_for_display;

// Reached only from this slice's own `callbacks.rs`, which used to live two
// modules away, plus the cross-slice `apply_row_*` mirrors in
// `callbacks::now_playing`. `pub(super)` is `pub(in crate::ui)` here, which is
// exactly that reach.
pub(super) use cards::{BrowseViewMode, mode_from_index, mode_index, rebuild_cards};
pub(super) use fetch::{apply_row_favorite, apply_row_rating, fetch_and_apply, resort_and_apply};
pub(super) use selection::{clear_selection, handle_select_row};

/// Install the Browse models, build the handle, wire every `Browse.*` callback,
/// and seed the persisted path + presentation mode.
///
/// The seed folds in here because it is Browse-local and needs only the handle
/// this call just built — and folding it is what let it take `views.json` as an
/// argument instead of re-reading the file `install_views` had already parsed.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<BrowseUi> {
    install_models(cx.app);
    install_selection_model(cx.app);
    let browse_ui = Arc::new(BrowseUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, &browse_ui);
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
    /// Last fetched files in display order (already sorted by the
    /// current `sort_field` / `sort_dir`) — used by `play-row`, the
    /// selection helpers, and the favourite toggle to recover full row
    /// data without round-tripping through the Slint model.
    pub(super) last_files: Mutex<Vec<BrowseFile>>,
    /// Last fetched subfolders, in the order they are published. Cached
    /// beside `last_files` because the card view draws both in one grid
    /// and a mode toggle rebuilds it with no fetch to take them from.
    pub(super) last_folders: Mutex<Vec<BrowseFolder>>,
    /// In-memory sort state. Browse can't push an `ORDER BY` to the DB
    /// (it mixes disk-only + DB files), so it re-sorts `last_files`.
    sort_field: Mutex<String>,
    sort_dir: Mutex<String>,
    /// Shared with Tracks/now-playing-bar so cached thumbnails are
    /// reused across views.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The card view's own 448 px tier — private, so releasing it can't
    /// yank the row thumbnails the shared tier above is still serving.
    /// Released on section-leave and whenever the view goes back to the
    /// list. See `cards.rs`.
    pub(super) grid_covers: Arc<CoverThumbs>,
    /// Synchronous shadow of `Browse.view-mode`, as a bool because the
    /// index lives only in Slint (`cards::mode_from_index`). The fetch
    /// reads it off a tokio worker, which is why it isn't read back off
    /// the global.
    card_mode: AtomicBool,
    /// Stale-fetch guard. See module-level comment.
    pub(super) fetch_token: AtomicU64,
    /// Visibility and staleness bookkeeping (`section-active-changed`
    /// shadow plus dirty flag). Browse releases nothing on leave —
    /// `dirty` is set only by the `library_changed` subscriber when a
    /// bump arrives while the section is hidden, and consumed on
    /// re-enter to re-fetch the current directory once.
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
                cards::GRID_COVER_SIZE,
                cards::DEFAULT_GRID_COVER_CAP,
            )),
            card_mode: AtomicBool::new(false),
            fetch_token: AtomicU64::new(0),
            section: SectionState::new(),
        }
    }

    /// Mirror the Browse-section-visible flag (`section-active-changed`).
    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    /// Whether the Browse section is currently on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Mark the cached directory listing stale — a `library_changed` bump
    /// arrived while the section was hidden. See [`Self::take_dirty`].
    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag. `true` on section-enter
    /// means a library mutation happened while hidden and the current
    /// directory must be re-fetched.
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
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
        self.card_mode
            .store(mode == BrowseViewMode::Card, Ordering::Relaxed);
    }

    /// Resolve one card's cover against the card tier, decoding only once the
    /// tier is known warm — `generation == 0` means "just toggled or just
    /// re-entered", so answer from the cache and let the card paint its
    /// placeholder rather than putting a 448 px decode per visible card on the
    /// UI thread.
    pub fn grid_cover(&self, artwork_path: &str, generation: i32) -> slint::Image {
        crate::ui::grid_prewarm::grid_cover(&self.grid_covers, artwork_path, generation)
    }

    /// Decode `paths` into the card tier. Blocking — call from `spawn_blocking`.
    ///
    /// Returns whether the tier is warm **and still holds what was decoded**: the
    /// answer a caller's `covers-generation` bump is gated on. `false` when a
    /// section leave or a toggle back to the list landed inside the decode, and
    /// there the buffers are handed straight back — announcing a tier the leave
    /// has already released puts the next cards on the decoding path. The two
    /// warm sites (a fetch, a mode toggle) differ only in where the paths come
    /// from, so the re-check lives here rather than at each of them.
    ///
    /// An empty `paths` is still a warm tier: a directory of nothing but
    /// subfolders has no cover to wait for, and its files may load on demand.
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
    /// caller, so `0` keeps meaning "this tier is cold" rather than "first toggle
    /// of the session". Called off the UI thread on a section-leave and whenever
    /// the view goes back to the list.
    pub fn release_grid_covers(&self) {
        self.grid_covers.clear();
        crate::tasks::heap_trim::trim();
    }

    pub fn current_path(&self) -> String {
        self.current_path.lock().clone()
    }

    /// Push `path` onto the back-stack and set `current_path` to `next`.
    /// `next` is the path the caller is navigating *to*; `path` is the
    /// path the caller is leaving.
    pub fn push_history(&self, leaving: String, going_to: String) {
        self.history.lock().push(leaving);
        *self.current_path.lock() = going_to;
    }

    /// Pop the most recent entry off the back-stack. Returns `None` when
    /// the stack is empty.
    pub fn pop_history(&self) -> Option<String> {
        let popped = self.history.lock().pop();
        if let Some(p) = popped.as_ref() {
            p.clone_into(&mut self.current_path.lock());
        }
        popped
    }

    /// Trim history down to (and including) `path` if it appears in the
    /// stack, then set `current_path` to `path`. If `path` isn't in the
    /// stack (a user clicked a breadcrumb that isn't an ancestor — rare
    /// but possible after a refresh), the stack is cleared.
    pub fn truncate_history_to(&self, path: &str) {
        let mut h = self.history.lock();
        if let Some(pos) = h.iter().position(|p| p == path) {
            h.truncate(pos);
        } else {
            h.clear();
        }
        path.clone_into(&mut self.current_path.lock());
    }

    /// Replace `current_path` without touching history. Used by `refresh`
    /// (no history change) and the initial seed.
    pub fn set_path(&self, path: String) {
        *self.current_path.lock() = path;
    }

    /// Current sort field (`"title"`, `"artist"`, …).
    pub fn sort_field(&self) -> String {
        self.sort_field.lock().clone()
    }

    /// Current sort direction (`"asc"` / `"desc"`).
    pub fn sort_dir(&self) -> String {
        self.sort_dir.lock().clone()
    }

    /// Store the sort field + direction. The next `fetch_and_apply` /
    /// `resort_and_apply` reads these.
    pub fn set_sort(&self, field: String, dir: String) {
        *self.sort_field.lock() = field;
        *self.sort_dir.lock() = dir;
    }

    /// IDs of `last_files` rows that are in the library, in display
    /// order. `play-row` passes the result to `player_play_tracks`.
    pub fn current_in_library_ids(&self) -> Vec<i64> {
        self.last_files
            .lock()
            .iter()
            .filter_map(|f| f.in_library.then_some(f.row.id))
            .collect()
    }

    /// Surgically flip `is_favorite` on the cached `last_files` row so a
    /// single-row favourite toggle doesn't need a full re-fetch.
    pub fn flip_favorite(&self, id: i64, fav: bool) {
        let mut files = self.last_files.lock();
        if let Some(f) = files.iter_mut().find(|f| f.row.id == id) {
            f.row.is_favorite = fav;
        }
    }

    /// Surgically set `rating` on the cached `last_files` row — the rating
    /// analogue of [`Self::flip_favorite`].
    pub fn flip_rating(&self, id: i64, rating: i32) {
        let mut files = self.last_files.lock();
        if let Some(f) = files.iter_mut().find(|f| f.row.id == id) {
            f.row.rating = rating;
        }
    }
}

/// Build empty `VecModel`s for `folders`, `rows`, `breadcrumbs` and the card
/// grid's `card-rows`, and hand them to the Slint `Browse` global as `ModelRc`s.
/// Subsequent updates locate them by downcasting back to `VecModel<T>` and
/// mutating in place.
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

/// Install a persistent `VecModel<i32>` for `Browse.selected-ids` so
/// selection mutations can `set_vec` into the same model instead of
/// allocating a fresh `ModelRc + VecModel` on every click. Mirrors
/// `tracks::install_selection_model`.
fn install_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Browse>().set_selected_ids(ModelRc::from(model));
}

/// Seed `Browse.current-path` and `Browse.view-mode` from `views.json` at
/// startup and kick the initial fetch. `None` — a fresh install, or a file that
/// wouldn't read — leaves `current-path` empty (root) and the mode on the list,
/// and the fetch lands at the root view.
///
/// Takes the boot read rather than repeating it: `install_views` already has
/// `views.json` in hand, and this used to be a second full parse of it.
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
    // written by a build with more presentations would otherwise select a branch
    // that mounts nothing.
    let mode_idx = crate::ui::tab_bar::clamp_tab(persisted_mode, g.get_view_mode_count());
    g.set_view_mode(mode_idx);
    browse_ui.set_view_mode(mode_from_index(&g, mode_idx));

    let weak = ui.as_weak();
    let state_clone = state.clone();
    let browse_clone = browse_ui.clone();
    state.runtime.spawn(async move {
        if let Err(e) =
            fetch_and_apply(&state_clone, &browse_clone, weak, initial_path).await
        {
            log::warn!("browse seed_from_settings fetch failed: {e}");
        }
    });
}

/// Re-export of `BreadcrumbRow` used by `install_browse_models`.
use crate::BreadcrumbRow as UiBreadcrumbRow;

/// Convert a `BrowseFile` into the shared `TrackListRow` the `TrackList`
/// component renders. In-library files reuse the Tracks-view converter
/// (full data, `enabled: true`); disk-only files become a sparse,
/// dimmed, non-interactive row (`id == 0`, `enabled: false`).
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

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<BrowseUi>();
};
