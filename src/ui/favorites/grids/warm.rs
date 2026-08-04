//! Which of the prepared hashes belongs to the tab on screen.
//!
//! The tab-agnostic half of the same question — folding the tab and the column
//! count in, and deciding whether a landed prewarm may announce itself — lives
//! in [`crate::ui::tab_bar`], since Recently Played asks it identically. What
//! stays here is the one part that names *these* tabs.
//!
//! Pure, and named rather than inlined, because its failure mode is a grid that
//! looks correct and is stale.

use super::apply::PreparedGrids;
use crate::ui::favorites::FavoritesTab;

/// The content hash of the tab that is actually on screen.
///
/// Only that one can be what changed visibly — the hidden grid's model is empty
/// either way. Folding both in would undo the whole point of hashing them apart:
/// every play-count flush moves Most Played's hash, and the Artists grid, which
/// shows nothing derived from a play count, would rebuild along with it.
pub(super) fn mounted_content(tab: FavoritesTab, prepared: &PreparedGrids) -> u64 {
    match tab {
        FavoritesTab::MostPlayed => prepared.most_played_content,
        FavoritesTab::Artists => prepared.artists_content,
        FavoritesTab::Songs => 0,
    }
}
