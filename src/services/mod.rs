//! The `services::…` paths the binary still spells, re-exported so each resolves whichever crate
//! now owns it.
//!
//! Five of the nine, and the four that are gone were dropped rather than kept for symmetry:
//! `net` is `melodia-net`'s and nothing here opens a socket, and `search_history`,
//! `diagnostics` and `artist_images` are reached from views rather than from `main.rs` or
//! `boot/`. The `net` line is what kept `melodia-net` on this package's manifest.

pub use melodia_app::services::{settings, updater, view_state};
pub use melodia_integrations::services::integrations;
pub use melodia_platform::services::platform;

// A walking pin rather than a module's own tests: it asks what every package format ships, what
// the gate's workflow waits on and how long a thread name may be, and answers nothing about
// `services` at all. It stays with the binary, which is where Phase D collects the corpus walks.
#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
