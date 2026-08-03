//! The two grid tabs — Most Played and Favorite Artists.
//!
//! Album-related state is gone by design: albums are reachable via the Albums
//! tab, and surfacing them again on Favorites was redundant.
//!
//! Neither query is capped: both tabs are virtualized `EntityCardGrid`s, so
//! the whole set is fetched once and only on-screen rows instantiate cards.
//! The cover prewarm *is* capped, at `GRID_PREWARM_AHEAD` — a screenful, not
//! the tier's capacity. Warming to capacity over an uncapped grid means the
//! prewarm evicts its own earlier work before a single card asks for one; the
//! rest decode lazily as rows scroll in.
//!
//! And only the mounted tab's tier, only while the section is on screen. The
//! two grids are mutually exclusive, so warming both is twice the decodes and
//! twice the resident buffers for a surface nobody can scroll. For the same
//! reason only the mounted tab's *model* is built — a hidden grid's rows are
//! `SharedString`s nobody can see.
//!
//! Both grids honour the hero filter: every fetch / apply path re-walks the
//! cached Rust Vecs through the current `Favorites.filter` needle (title +
//! artist for Most Played, name for Favorite Artists) before writing the
//! Slint models.
//!
//! Split four ways, because the four answer different questions: [`fetch`]
//! runs the queries, [`apply`] turns caches into models, [`warm`] decides
//! whether an apply or a cover announcement may happen at all, and [`sort`]
//! owns the Favorite Artists order.

mod apply;
mod fetch;
mod sort;
mod warm;

pub use apply::{apply_filtered_grids_now, mark_covers_warm};
pub use fetch::refresh_grids;
pub use sort::set_artist_sort;

#[cfg(test)]
#[path = "../tests/grids_tests.rs"]
mod tests;
