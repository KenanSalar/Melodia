//! OS and I/O adapters, in the four groups that become four crates.
//!
//! - [`net`]: the shared HTTP primitives, and the radio directory client over them.
//! - [`platform`]: everything that answers to the OS rather than to a feature.
//! - [`integrations`]: the three surfaces outside the app that get told what is playing.
//! - The rest, flat here: settings, the two JSON state files, the artist-image orchestration, the
//!   diagnostics bundle and the updater. Each of them names `database` or `state` or both, so
//!   they are app-level rather than adapters, and they are what is left once the three groups go.

pub mod integrations;
pub mod net;
pub mod platform;

pub mod artist_images;
pub mod diagnostics;
pub mod search_history;
pub mod settings;
pub mod updater;
pub mod view_state;

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
