//! Everything that reads or writes a media file, in three tiers that do not depend on each other
//! symmetrically.
//!
//! - [`image`]: decode, resize, the content-addressed artwork store, the thumbnail cache and the
//!   palette extraction over it. **No outbound `crate::` edge at all**, which is what makes it the
//!   one tier the others may lean on.
//! - [`ingest`]: the scanner, the tag reader and writer, and the filesystem watcher. Reads
//!   `image` for covers, and holds the directory's one edge into `player`
//!   (`ingest::metadata`'s duration probe for the containers lofty can't measure).
//! - [`fetch`]: the four things that open a socket for artwork or a station logo. Reads `image` to
//!   store what it got, and the shared HTTP primitives to get it.
//!
//! The tiers land in three different crates, so the direction matters more than the grouping:
//! `image` below `ingest` and `fetch`, and nothing pointing back up.

pub use melodia_artwork::media::image;
pub use melodia_net::media::fetch;

pub mod ingest;

// A walking pin rather than a module's own tests: what it asks is where in `src/` a lofty parse
// may start, which no one file in here is positioned to answer.
#[cfg(test)]
#[path = "tests/lofty_open_tests.rs"]
mod lofty_open_tests;
