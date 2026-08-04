//! Which of the prepared hashes belongs to the tab on screen.
//!
//! The tab-agnostic half of the same question — folding the tab and the column
//! count in, and deciding whether a landed prewarm may announce itself — lives
//! in [`crate::ui::tab_bar`], since Favorites asks it identically. What stays
//! here is the one part that names *these* tabs.
//!
//! Pure, and named rather than inlined, because its failure mode is a grid that
//! looks correct and is stale.

use super::apply::PreparedGrid;
use crate::ui::recently_played::RecentlyPlayedTab;

/// The content hash of the tab that is actually on screen.
///
/// Only that one can be what changed visibly — the Songs tab mounts no grid, so
/// nothing the grid could rebuild is on screen there. A constant `0` rather than
/// the grid's own hash is what stops a `stats_changed` tick, which reaches both
/// tabs, forcing a write nobody can see.
pub(super) fn mounted_content(tab: RecentlyPlayedTab, prepared: &PreparedGrid) -> u64 {
    match tab {
        RecentlyPlayedTab::MostPlayed => prepared.most_played_content,
        RecentlyPlayedTab::Songs => 0,
    }
}
