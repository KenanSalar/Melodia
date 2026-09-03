//! The image tier, which is the whole of `media/` the binary reaches.
//!
//! Its two siblings are crates of their own now and neither is re-exported here, because nothing
//! left in this package names them: `ingest` (the scanner, the tag reader and writer, the watcher)
//! is `melodia-store`'s and reached through `library`, and `fetch` (the four things that open a
//! socket for artwork or a logo) is `melodia-net`'s and reached through `tasks`. Dropping the two
//! lines is what takes `melodia-net` off this package's manifest.
//!
//! The direction between the three has not changed and is the reason they are three crates:
//! `image` has no outbound `crate::` edge at all, the other two read it, and nothing points back
//! up.

pub use melodia_artwork::media::image;

// A walking pin rather than a module's own tests: what it asks is where in the workspace a lofty
// parse may start, which no one file in any of the three tiers is positioned to answer.
#[cfg(test)]
#[path = "tests/lofty_open_tests.rs"]
mod lofty_open_tests;
