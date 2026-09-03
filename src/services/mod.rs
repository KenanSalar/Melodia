//! The four groups as the binary reaches them, re-exported so `crate::services::…` resolves
//! whichever crate now owns a module.

pub use melodia_app::services::{
    artist_images, diagnostics, search_history, settings, updater, view_state,
};
pub use melodia_integrations::services::integrations;
pub use melodia_net::services::net;
pub use melodia_platform::services::platform;

// A walking pin rather than a module's own tests: it asks what every package format ships, what
// the gate's workflow waits on and how long a thread name may be, and answers nothing about
// `services` at all. It stays with the binary, which is where Phase D collects the corpus walks.
#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
