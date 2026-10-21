//! The Rust half of `crates/melodia-ui/ui/components/tab-bar.slint`.
//!
//! Every page that mounts a `TabBar` persists which tab was showing, so each needs the
//! same read-side clamp; the component's source-level invariants are pinned here too, no
//! host owning the file. The grid-bearing pages also share the count sentinel below.

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

#[cfg(test)]
#[path = "tests/tab_bar_tests.rs"]
mod tests;
