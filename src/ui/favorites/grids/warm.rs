//! The three predicates deciding whether an apply — or a cover announcement —
//! may happen at all.
//!
//! Pure, and named rather than inlined, because each one's failure mode is a
//! grid that looks correct and is stale.

use std::hash::{DefaultHasher, Hash, Hasher};

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

/// Fold the mounted tab and the column count into the content hash.
///
/// Both shape what is on screen independently of the data: a tab switch has to
/// fill one model and empty the other, and a column change re-chunks the same
/// cards into different rows. Leave either out and the apply that needs to run
/// most is the one that gets skipped.
pub(super) fn grid_signature(tab: FavoritesTab, columns: i32, content: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    tab.hash(&mut hasher);
    columns.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

/// Whether a landed prewarm may announce its tier to the cards.
///
/// `warmed` is the tab `fetch::refresh_grids` decoded for *and still holds the
/// buffers of* — `None` when it skipped the prewarm because the section was
/// already hidden, and equally when the decode ran and a leave landing inside
/// it took the buffers straight back. The other two
/// are read on the UI thread, where both shadows are written, so this is the
/// same re-check `on_tab_changed` makes after *its* `swap_tab_covers`: a leave
/// has rewound the counter and dropped the buffers, and a tab pick that
/// overtook the decodes owns a different tier entirely — announcing either
/// would put the next surface's cards straight back on the decoding path.
///
/// Deliberately *not* a function of whether the rows changed. Those are
/// independent facts, and conflating them is what left the Most Played grid on
/// placeholders after a section re-enter: the mount-time `columns-changed`
/// apply had already written the final rows by the time the prewarm returned,
/// so the write that carried the announcement was skipped as a no-op repaint
/// and the counter stayed at its cold 0 until the next tab pick.
pub(super) fn should_announce_warm(
    warmed: Option<FavoritesTab>,
    section_active: bool,
    current_tab: FavoritesTab,
) -> bool {
    section_active && warmed == Some(current_tab)
}
