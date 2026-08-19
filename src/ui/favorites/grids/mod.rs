//! The two grid tabs — Most Played and Favorite Artists.
//!
//! Albums are the Albums tab's; surfacing them again here was redundant.
//!
//! Neither query is capped, only on-screen rows instantiating cards; the cover
//! prewarm is capped at `GRID_PREWARM_AHEAD` because warming to a tier's
//! capacity over an uncapped grid evicts its own earlier work before a card asks
//! for one. Only the mounted tab's tier and model, and only while the section is
//! on screen — the two grids are mutually exclusive, so the hidden one is
//! decodes and `SharedString`s nobody can scroll to.
//!
//! Every fetch / apply path re-walks the cached `Vec`s through the current
//! `Favorites.filter` (title + artist for Most Played, name for Favorite
//! Artists) before writing the models.
//!
//! [`fetch`] runs the queries, [`apply`] turns caches into models, [`warm`]
//! decides whether an apply or a cover announcement may happen at all, and
//! [`sort`] owns the Favorite Artists order.

mod apply;
mod fetch;
mod sort;
mod warm;

pub use apply::{apply_filtered_grids_now, mark_covers_warm, repaint_covers};
pub use fetch::refresh_grids;
pub use sort::set_artist_sort;

#[cfg(test)]
#[path = "../tests/grids_tests.rs"]
mod tests;
