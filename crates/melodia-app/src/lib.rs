//! What the app decides to do, over every tier that can carry it out.
//!
//! The library API the UI reaches the database through, the background tasks, the state the
//! callbacks share and the settings behind all three. It names every crate below it, which is
//! what makes it the command layer rather than another tier.
//!
//! **The facade below is `pub` only for what `melodia-views` may reach.** A re-export launders a
//! dependency past a manifest, so `database`, `media::ingest`, `media::fetch`, `services::net`
//! and `player::source` are `pub(crate)`: views depends on this crate and must not get a socket
//! or the schema through it.

pub use melodia_core::{config, entities, error, themes, utils};
// The two generated types `tasks::updater_daily` drives the update banner through. An item list
// rather than the binary's glob, one task being the whole of what this crate draws.
pub use melodia_ui::{AppWindow, MelodiaUpdater};

pub(crate) use melodia_store::database;

pub mod library;
pub mod services;
pub mod state;
pub mod tasks;

pub mod media {
    pub use melodia_artwork::media::image;

    pub(crate) use melodia_net::media::fetch;
    pub(crate) use melodia_store::media::ingest;
}

pub mod player {
    pub use melodia_engine::player::engine;
    pub use melodia_playback::player::playback;

    pub(crate) use melodia_audio::player::source;
}

#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
