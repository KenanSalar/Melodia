//! The Most Played tab — a virtualized `EntityCardGrid` over the library's
//! played tracks, ranked by count.
//!
//! The query is uncapped, only on-screen rows instantiating cards; the cover
//! prewarm is capped at `GRID_PREWARM_AHEAD` because warming to a tier's capacity
//! over an uncapped grid evicts its own earlier work before a card asks for one.
//! Both run only while this tab is mounted and the section on screen — the two
//! tabs are mutually exclusive, so a hidden grid holds rows nobody can see.
//!
//! Every fetch / apply path re-walks the cached `Vec` through the current
//! `RecentlyPlayed.filter` before writing the model, on the same
//! [`crate::ui::row_match::most_played_matches`] the Songs list beside it runs.
//!
//! [`fetch`] runs the query, [`apply`] turns the cache into a model, [`warm`]
//! names which prepared hash belongs to the tab on screen. No `sort` sibling —
//! this tab's title states its own ordering.

mod apply;
mod fetch;
mod warm;

pub use apply::{apply_filtered_grid_now, apply_filtered_grid_settled, mark_covers_warm};
pub use fetch::refresh_grid;

#[cfg(test)]
#[path = "../tests/grid_tests.rs"]
mod tests;
