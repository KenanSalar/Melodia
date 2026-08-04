//! Browse-view glue between Rust and Slint.
//!
//! The Slint `Browse` global owns three `Rc<VecModel<_>>` lists
//! (`folders`, `rows`, `breadcrumbs`) installed once at startup via
//! [`install_browse_models`], plus a persistent `VecModel<i32>` for the
//! selection ([`install_browse_selection_model`]). [`fetch_and_apply`]
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
mod fetch;
mod models;
mod selection;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::browse::BrowseFile;
use crate::media::cover_thumbs::CoverThumbs;
use crate::services::view_state;
use crate::ui::section_state::SectionState;
use crate::state::AppState;
use crate::{
    AppWindow, Browse, BrowseFolderRow as UiBrowseFolderRow, TrackListRow as UiTrackListRow,
};

pub use fetch::{apply_row_favorite, apply_row_rating, fetch_and_apply, resort_and_apply};
pub use selection::{clear_selection, handle_select_row};

/// Rust-side state for the Browse view. Shared between the UI callbacks
/// (`wire_browse`) and the async fetcher.
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
    /// In-memory sort state. Browse can't push an `ORDER BY` to the DB
    /// (it mixes disk-only + DB files), so it re-sorts `last_files`.
    sort_field: Mutex<String>,
    sort_dir: Mutex<String>,
    /// Shared with Tracks/now-playing-bar so cached thumbnails are
    /// reused across views.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
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
    pub fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            current_path: Mutex::new(String::new()),
            history: Mutex::new(Vec::new()),
            last_files: Mutex::new(Vec::new()),
            sort_field: Mutex::new("title".to_owned()),
            sort_dir: Mutex::new("asc".to_owned()),
            cover_thumbs,
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

/// Build empty `VecModel`s for `folders`, `rows`, and `breadcrumbs` and
/// hand them to the Slint `Browse` global as `ModelRc`s. Subsequent
/// updates locate them by downcasting back to `VecModel<T>` and mutating
/// in place.
pub fn install_browse_models(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    let folders: Rc<VecModel<UiBrowseFolderRow>> = Rc::new(VecModel::default());
    let rows: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    let crumbs: Rc<VecModel<UiBreadcrumbRow>> = Rc::new(VecModel::default());
    g.set_folders(ModelRc::from(folders));
    g.set_rows(ModelRc::from(rows));
    g.set_breadcrumbs(ModelRc::from(crumbs));
}

/// Install a persistent `VecModel<i32>` for `Browse.selected-ids` so
/// selection mutations can `set_vec` into the same model instead of
/// allocating a fresh `ModelRc + VecModel` on every click. Mirrors
/// `tracks::install_selection_model`.
pub fn install_browse_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Browse>().set_selected_ids(ModelRc::from(model));
}

/// Seed `Browse.current-path` from `views.json`'s `browse_path` at startup
/// and kick the initial fetch. Missing / unreadable file leaves
/// `current-path` empty (root) and the fetch lands at the root view.
pub fn seed_from_settings(ui: &AppWindow, state: &AppState, browse_ui: &Arc<BrowseUi>) {
    let initial_path = view_state::read_view_state(&state.paths)
        .ok()
        .and_then(|s| s.browse_path)
        .unwrap_or_default();
    browse_ui.set_path(initial_path.clone());
    ui.global::<Browse>().set_current_path(SharedString::from(initial_path.as_str()));

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
