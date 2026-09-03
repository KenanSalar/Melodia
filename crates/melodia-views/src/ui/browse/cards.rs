//! Browse's card view — the flat card list, its chunk into grid rows, and the
//! grid-tier cover cache behind it.
//!
//! The grid draws subfolders and files together, folders first and then files in
//! the list's own display order, so flipping the mode re-presents exactly what the
//! `TrackList` was showing. `Browse.folders` and `Browse.rows` stay populated in
//! both modes — the view's three empty states count them — and only `card-rows` is
//! mode-gated: rows built for the surface that isn't mounted reach nothing (the
//! `favorites::grids::apply::build_filtered_grids` precedent).
//!
//! The cover tier here is **private and grid-sized**, never the shared row tier
//! `BrowseUi::cover_thumbs` points at: the two hold buffers an order of magnitude
//! apart in size, so mixing them would evict the row thumbnails Tracks and the
//! now-playing bar are also using.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use slint::{ComponentHandle, SharedString};

use super::BrowseUi;
use crate::ui::grid_prewarm;
use crate::ui::grid_rows::{chunk_built_rows, write_grid};
use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use melodia_core::entities::browse::{BrowseFile, BrowseFolder};
use melodia_ui::{
    AppWindow, Browse, BrowseCardGridRow as UiBrowseCardGridRow, BrowseCardRow as UiBrowseCardRow,
};

/// Fallback LRU capacity for the card cover cache, used at construction and
/// whenever the display can't be queried. [`tune_cache_for_display`] replaces it
/// at startup with a resolution-derived cap. Must stay ≥ [`GRID_PREWARM_AHEAD`]
/// or the prewarm would thrash the LRU it fills.
pub(super) const DEFAULT_GRID_COVER_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("DEFAULT_GRID_COVER_CAP > 0"),
};

/// How many leading cards' covers a fetch or a mode toggle prewarms before the
/// grid paints. Roughly one screenful at any reasonable column count; everything
/// past it decodes lazily on scroll-in through `request-card-cover`.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;

/// Which presentation the Browse view is drawing.
///
/// The *indices* live in `globals/tracks.slint`'s `mode-*` constants and no Rust
/// file restates them — [`mode_from_index`] and [`mode_index`] resolve one against
/// the global, on the UI thread, which is the only place it is reachable. The
/// shadow on [`BrowseUi`] carries the enum instead, because the fetch that reads
/// it runs on a tokio worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowseViewMode {
    List,
    Card,
}

impl BrowseViewMode {
    /// The other one. The toggle pill means "switch", so Rust negates rather than
    /// taking a mode from Slint.
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::List => Self::Card,
            Self::Card => Self::List,
        }
    }
}

/// Resolve a `Browse.view-mode` value against the global's own `mode-*` constants.
pub fn mode_from_index(g: &Browse<'_>, idx: i32) -> BrowseViewMode {
    if idx == g.get_mode_card() {
        BrowseViewMode::Card
    } else {
        BrowseViewMode::List
    }
}

/// The `Browse.view-mode` value for `mode` — [`mode_from_index`] backwards.
pub fn mode_index(g: &Browse<'_>, mode: BrowseViewMode) -> i32 {
    match mode {
        BrowseViewMode::List => g.get_mode_list(),
        BrowseViewMode::Card => g.get_mode_card(),
    }
}

/// Build the flat card list: every subfolder, then every file in display order.
///
/// A track card carries `row_index`, its slot in `Browse.rows`, so activating one
/// reaches `play-row` with exactly the index the `TrackList` would have passed —
/// including for a disk-only file, which sits in the displayed list but not in
/// `current_in_library_ids` and is what `play_row_start`'s lookup-by-id fallback
/// exists for. A folder card's subtitle is left empty and supplied as a literal by
/// the grid delegate: `@tr` only translates literals at codegen, so a string Rust
/// pushed would render untranslated.
pub fn to_browse_card_rows(folders: &[BrowseFolder], files: &[BrowseFile]) -> Vec<UiBrowseCardRow> {
    let mut cards = Vec::with_capacity(folders.len() + files.len());
    cards.extend(folders.iter().map(|f| UiBrowseCardRow {
        id: 0,
        row_index: -1,
        path: SharedString::from(f.path.as_str()),
        title: SharedString::from(f.name.as_str()),
        subtitle: SharedString::from(""),
        artwork_path: SharedString::from(""),
        is_folder: true,
        enabled: true,
    }));
    cards.extend(files.iter().enumerate().map(|(idx, f)| UiBrowseCardRow {
        id: if f.in_library {
            clamp_i64_to_i32(f.row.id)
        } else {
            0
        },
        row_index: len_as_i32(idx),
        path: SharedString::from(""),
        title: SharedString::from(f.row.title.as_str()),
        subtitle: SharedString::from(f.row.artist.as_deref().unwrap_or("")),
        artwork_path: SharedString::from(f.row.artwork_path.as_deref().unwrap_or("")),
        is_folder: false,
        enabled: f.in_library,
    }));
    cards
}

/// Rebuild `Browse.card-rows` from the cached folder + file lists. UI thread, no
/// DB — the fetch, the mode toggle and a column change all land here.
///
/// Empties the model outright while the list is mounted, so the unmounted surface
/// holds no rows and a later toggle can't paint a stale directory for a frame.
pub fn rebuild_cards(ui: &AppWindow, browse_ui: &BrowseUi) {
    let g = ui.global::<Browse>();
    if browse_ui.view_mode() != BrowseViewMode::Card {
        write_grid(&g.get_card_rows(), Vec::new(), "browse::cards");
        return;
    }
    let cards = {
        let folders = browse_ui.last_folders.lock();
        let files = browse_ui.last_files.lock();
        to_browse_card_rows(&folders, &files)
    };
    let rows = chunk_built_rows(cards, g.get_columns(), |cards| UiBrowseCardGridRow { cards });
    write_grid(&g.get_card_rows(), rows, "browse::cards");
}

/// Artwork paths for roughly the first screenful of cards, in display order.
///
/// Folders carry no artwork, so they are skipped rather than counted — a
/// directory of nothing but subfolders prewarms nothing, and one with a few
/// leading subfolders still warms a full screenful of the files behind them.
pub fn first_screenful_paths(files: &[BrowseFile]) -> Vec<PathBuf> {
    grid_prewarm::unique_artwork_paths(
        files.iter().map(|f| f.row.artwork_path.as_deref()),
        GRID_PREWARM_AHEAD,
    )
}

/// Re-run the mounted card bindings once a scheduled decode has landed — the
/// `favorites::grids::apply::repaint_covers` contract, never moving off 0.
pub fn repaint_covers(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    let generation = g.get_covers_generation();
    if generation > 0 {
        g.set_covers_generation(generation.saturating_add(1));
    }
}

/// Size the card cover cache against the display it will be drawn on. Called after `app.show()`
/// and again on every resize, off `WindowChrome.display-changed`.
pub fn tune_cache_for_display(app: &AppWindow, browse_ui: &BrowseUi) {
    let cap = grid_prewarm::cover_cap_for_window(app, DEFAULT_GRID_COVER_CAP);
    browse_ui.grid_covers.resize(cap);
    browse_ui.grid_covers.set_thumb_size(grid_prewarm::cover_size_for_window(app));
}

#[cfg(test)]
#[path = "tests/cards_tests.rs"]
mod tests;
