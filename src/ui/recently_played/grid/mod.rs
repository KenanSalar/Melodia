//! The Most Played tab — a virtualized `EntityCardGrid` over the library's
//! played tracks, ranked by count.
//!
//! The query is uncapped: the tab is an `EntityCardGrid`, so the whole set is
//! fetched once and only on-screen rows instantiate cards. The cover prewarm
//! *is* capped, at `GRID_PREWARM_AHEAD` — a screenful, not the tier's capacity.
//! Warming to capacity over an uncapped grid means the prewarm evicts its own
//! earlier work before a single card asks for one; the rest decode lazily as
//! rows scroll in.
//!
//! And only while this tab is the mounted one, and only while the section is on
//! screen. The two tabs are mutually exclusive, so a hidden grid's rows are
//! `SharedString`s nobody can see and its tier is resident buffers nobody can
//! scroll.
//!
//! The grid honours the hero filter: every fetch / apply path re-walks the
//! cached Rust `Vec` through the current `RecentlyPlayed.filter` needle before
//! writing the Slint model — the same [`crate::ui::row_match::most_played_matches`]
//! predicate the Songs list beside it runs.
//!
//! Split three ways, because the three answer different questions: [`fetch`]
//! runs the query, [`apply`] turns the cache into a model, and [`warm`] names
//! which prepared hash belongs to the tab on screen. There is no `sort`
//! sibling — this tab's title states its own ordering.

mod apply;
mod fetch;
mod warm;

pub use apply::{apply_filtered_grid_now, apply_filtered_grid_settled, mark_covers_warm};
pub use fetch::refresh_grid;

#[cfg(test)]
#[path = "../tests/grid_tests.rs"]
mod tests;
