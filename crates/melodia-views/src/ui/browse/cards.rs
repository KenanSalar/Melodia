//! Browse's card view: folders first, then files in the list's display order, so a mode flip
//! re-presents what the `TrackList` showed. `Browse.folders` and `Browse.rows` stay filled in both
//! modes, the three empty states counting them; only `card-rows` follows the mode.
//!
//! Cards draw from `grid_prewarm::tier()`, never the row tier: their buffers are far larger, and
//! sharing would evict the thumbnails Tracks and the now-playing bar use.

use std::path::PathBuf;

use slint::{ComponentHandle, SharedString};

use super::BrowseUi;
use crate::ui::grid_prewarm;
use crate::ui::grid_rows::chunk_built_rows;
use crate::ui::model_diff::replace_rows;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32, opt_shared};
use melodia_core::entities::browse::{BrowseFile, BrowseFolder};
use melodia_ui::{
    AppWindow, Browse, BrowseCardGridRow as UiBrowseCardGridRow, BrowseCardRow as UiBrowseCardRow,
};

/// Which presentation Browse draws. The indices are `globals/tracks.slint`'s `mode-*` constants,
/// which no Rust file restates.
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

/// Resolves a `Browse.view-mode` value against the global's own `mode-*` constants.
pub fn mode_from_index(g: &Browse<'_>, idx: i32) -> BrowseViewMode {
    if idx == g.get_mode_card() { BrowseViewMode::Card } else { BrowseViewMode::List }
}

/// The `Browse.view-mode` value for `mode` — [`mode_from_index`] backwards.
pub fn mode_index(g: &Browse<'_>, mode: BrowseViewMode) -> i32 {
    match mode {
        BrowseViewMode::List => g.get_mode_list(),
        BrowseViewMode::Card => g.get_mode_card(),
    }
}

/// The flat card list. A track card carries `row_index`, its slot in `Browse.rows`, so it reaches
/// `play-row` with the index the `TrackList` would pass. A folder card's subtitle is left to the
/// grid delegate's `@tr` literal, a string from Rust going untranslated.
pub fn to_browse_card_rows(folders: &[BrowseFolder], files: &[BrowseFile]) -> Vec<UiBrowseCardRow> {
    let mut cards = Vec::with_capacity(folders.len() + files.len());
    cards.extend(folders.iter().map(|f| UiBrowseCardRow {
        id: 0,
        row_index: -1,
        path: SharedString::from(f.path.as_str()),
        title: SharedString::from(f.name.as_str()),
        subtitle: SharedString::default(),
        artwork_path: SharedString::default(),
        is_folder: true,
        enabled: true,
    }));
    cards.extend(files.iter().enumerate().map(|(idx, f)| UiBrowseCardRow {
        id: if f.in_library { clamp_i64_to_i32(f.row.id) } else { 0 },
        row_index: len_as_i32(idx),
        path: SharedString::default(),
        title: SharedString::from(f.row.title.as_str()),
        subtitle: opt_shared(f.row.artist.as_deref()),
        artwork_path: opt_shared(f.row.artwork_path.as_deref()),
        is_folder: false,
        enabled: f.in_library,
    }));
    cards
}

/// Rebuilds `Browse.card-rows` from the cached lists, or empties it while the list is mounted, so
/// a later toggle can't paint a stale directory for a frame.
pub fn rebuild_cards(ui: &AppWindow, browse_ui: &BrowseUi) {
    let g = ui.global::<Browse>();
    if browse_ui.view_mode() != BrowseViewMode::Card {
        replace_rows(&g.get_card_rows(), Vec::new(), "browse::cards");
        return;
    }
    let cards = {
        let folders = browse_ui.last_folders.lock();
        let files = browse_ui.last_files.lock();
        to_browse_card_rows(&folders, &files)
    };
    let rows = chunk_built_rows(cards, g.get_columns(), |cards| UiBrowseCardGridRow { cards });
    replace_rows(&g.get_card_rows(), rows, "browse::cards");
}

/// The artwork of roughly the first screenful of cards, in display order. Folders have none and
/// aren't counted, so leading subfolders don't shorten the screenful.
pub fn first_screenful_paths(files: &[BrowseFile]) -> Vec<PathBuf> {
    grid_prewarm::unique_artwork_paths(
        files.iter().map(|f| f.row.artwork_path.as_deref()),
        grid_prewarm::GRID_PREWARM_AHEAD,
    )
}

#[cfg(test)]
#[path = "tests/cards_tests.rs"]
mod tests;
