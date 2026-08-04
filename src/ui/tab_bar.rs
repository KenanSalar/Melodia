//! The Rust half of `melodia-ui/ui/components/tab-bar.slint`.
//!
//! Three pages mount a `TabBar` and persist which tab was showing — Settings
//! (`SettingsPage.tab-idx`, `views.json`'s `settings_tab`), Favorites
//! (`Favorites.tab-idx`, `favorites_tab`) and Recently Played
//! (`RecentlyPlayed.tab-idx`, `recently_played_tab`). All three need the same
//! read-side clamp, and the component's source-level invariants are pinned here
//! rather than under any host, since none owns it.
//!
//! The two grid-bearing pages additionally share the pair of predicates below,
//! plus the count sentinel. The predicates are about a *tab*, not about what a
//! tab contains, which is what makes them generic over the host's own tab enum
//! rather than duplicated per view; the sentinel is about a curated page's
//! teardown, and lives here because those two pages are its only users.

use std::hash::{DefaultHasher, Hash, Hasher};

/// What a curated page's card / track counts hold before anything has been
/// fetched, and again after a section leave has emptied the models they number.
///
/// Every one of those counts gates an *empty state* (`== 0`) or an action pill
/// (`> 0`), so leaving a real value behind across a leave suppresses a
/// placeholder over a model that really is empty — a derived value outliving its
/// source, the rule `songs_fold` and `most_played_totals` already follow. Writing
/// `0` instead is the opposite bug and the louder one: every re-enter would
/// assert "No favorites yet" for as long as the re-fetch takes, and on the Most
/// Played tabs that fetch waits on a cover-decode burst.
///
/// `-1` satisfies neither comparison, so every gate keeps the expression it
/// already had and simply stays quiet until there is an answer. The one reader
/// that has to know is `MosaicHeroTile`, which splits on *both* and would render
/// nothing in between — its two mounts clamp with `max(…, 0)` so the square
/// shows its empty glyph, matching the `mosaic-paths` model cleared beside it.
pub(crate) const UNFETCHED_COUNT: i32 = -1;

// The sentinel's whole job is to satisfy neither gate, which for an `i32` is
// exactly "negative". A compile-time assertion rather than a test, because a
// value that failed it would be a contradiction in terms rather than a
// regression to catch on the next run.
const _: () = assert!(
    UNFETCHED_COUNT < 0,
    "UNFETCHED_COUNT must miss both the `== 0` empty-state gates and the `> 0` pill gates"
);

/// Clamp a persisted tab index into range.
///
/// `tab_count` comes from the mounting page's Slint global rather than a const
/// here, so the number of tabs has exactly one definition — in the Slint that
/// declares them.
///
/// The guard matters on read, not write: the tab bar can only ever produce a
/// valid index, but a `views.json` left by a build with more tabs would
/// otherwise select a branch that mounts nothing and show a blank page.
pub(crate) fn clamp_tab(tab: i32, tab_count: i32) -> i32 {
    tab.clamp(0, (tab_count - 1).max(0))
}

/// Fold the mounted tab and the column count into a grid's content hash.
///
/// Both shape what is on screen independently of the data: a tab switch has to
/// fill one model and empty the other, and a column change re-chunks the same
/// cards into different rows. Leave either out and the apply that needs to run
/// most is the one that gets skipped.
///
/// `content` is the caller's own per-tab hash — this only decides what is folded
/// *around* it, which is the half neither host has any reason to spell twice.
pub(crate) fn grid_signature<T: Hash>(tab: T, columns: i32, content: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    tab.hash(&mut hasher);
    columns.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

/// Whether a landed prewarm may announce its tier to the cards.
///
/// `warmed` is the tab the fetch decoded for *and still holds the buffers of* —
/// `None` when it skipped the prewarm because the section was already hidden,
/// and equally when the decode ran and a leave landing inside it took the
/// buffers straight back. The other two are read on the UI thread, where both
/// shadows are written, so this is the same re-check a tab-change handler makes
/// after its own `swap_tab_covers`: a leave has rewound the counter and dropped
/// the buffers, and a tab pick that overtook the decodes owns a different tier
/// entirely — announcing either would put the next surface's cards straight back
/// on the decoding path.
///
/// Deliberately *not* a function of whether the rows changed. Those are
/// independent facts, and conflating them is what left a grid on placeholders
/// after a section re-enter: the mount-time `columns-changed` apply had already
/// written the final rows by the time the prewarm returned, so the write that
/// carried the announcement was skipped as a no-op repaint and the counter
/// stayed at its cold 0 until the next tab pick.
pub(crate) fn should_announce_warm<T: PartialEq + Copy>(
    warmed: Option<T>,
    section_active: bool,
    current_tab: T,
) -> bool {
    section_active && warmed == Some(current_tab)
}

#[cfg(test)]
#[path = "tests/tab_bar_tests.rs"]
mod tests;
