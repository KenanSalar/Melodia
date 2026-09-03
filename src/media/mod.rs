pub mod artwork;
pub mod cover_thumbs;
pub mod deezer;
pub mod image_decode;
pub mod itunes;
pub mod logo_discovery;
pub mod logo_tile;
pub mod metadata;
pub mod rating_tags;
pub mod scanner;
pub mod station_logo;
pub mod tag_writer;
pub mod watcher;

// A walking pin rather than a module's own tests: what it asks is where in `src/` a lofty parse
// may start, which no one file in here is positioned to answer.
#[cfg(test)]
#[path = "tests/lofty_open_tests.rs"]
mod lofty_open_tests;
