//! The Rust half of `melodia-ui/ui/components/tab-bar.slint`.
//!
//! Every page that mounts a `TabBar` persists which tab was showing, so each needs the
//! same read-side clamp; the component's source-level invariants are pinned here too, no
//! host owning the file. The two grid-bearing pages also share the pair of predicates
//! below plus the count sentinel — those are about a *tab* rather than what one contains,
//! which is what makes them generic over each host's own tab enum.

use std::hash::{DefaultHasher, Hash, Hasher};

/// What a curated page's card / track counts hold before anything is fetched, and again
/// after a section leave empties the models they number.
///
/// Every one gates an empty state (`== 0`) or an action pill (`> 0`). Leaving a real value
/// behind suppresses a placeholder over a model that really is empty; writing `0` is the
/// louder opposite, asserting "No favorites yet" for as long as the re-fetch takes. `-1`
/// satisfies neither, so every gate keeps the expression it already had. `MosaicHeroTile`
/// is the one reader that splits on both, and its two mounts clamp.
pub(crate) const UNFETCHED_COUNT: i32 = -1;

// Satisfying neither gate is, for an `i32`, exactly "negative". Compile-time rather than a
// test: a value that failed it is a contradiction in terms.
const _: () = assert!(
    UNFETCHED_COUNT < 0,
    "UNFETCHED_COUNT must miss both the `== 0` empty-state gates and the `> 0` pill gates"
);

/// Clamp a persisted tab index into range. `tab_count` comes from the mounting page's
/// Slint global rather than a const here, so the number of tabs has exactly one
/// definition. The guard matters on read, not write: the bar can only produce a valid
/// index, but a `views.json` left by a build with more tabs selects a branch that mounts
/// nothing.
pub(crate) fn clamp_tab(tab: i32, tab_count: i32) -> i32 {
    tab.clamp(0, (tab_count - 1).max(0))
}

/// Fold the mounted tab and the column count into a grid's content hash. Both shape what
/// is on screen independently of the data — a tab switch fills one model and empties the
/// other, a column change re-chunks the same cards — so leaving either out skips the apply
/// that most needed to run. `content` is the caller's own per-tab hash.
pub(crate) fn grid_signature<T: Hash>(tab: T, columns: i32, content: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    tab.hash(&mut hasher);
    columns.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

/// Whether a landed prewarm may announce its tier to the cards. `warmed` is the tab the
/// fetch decoded for *and still holds the buffers of* — `None` both when it skipped the
/// prewarm and when a leave landing inside the decode took them straight back. Announcing
/// under a rewound counter, or under a tab pick that overtook the decodes, puts the next
/// surface's cards back on the decoding path.
///
/// Deliberately *not* a function of whether the rows changed. Those are independent facts,
/// and conflating them left a grid on placeholders after a re-enter whose mount-time
/// `columns-changed` had already written the final rows.
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
