//! UI installation phase: chrome, per-view installers, settings hydration, initial fetches,
//! and the backend-to-UI subscribers.
//!
//! Split four ways by topic — the file held all four under one doc line and had grown past
//! seven hundred. Everything is re-exported, so `boot::ui_setup::*` is unchanged for callers.

pub mod chrome;
pub mod hydrate;
pub mod subscribers;
pub mod views;

pub use chrome::{
    apply_backdrop_style, install_app_chrome, install_backdrop_dither, install_locale,
};
pub use hydrate::{
    hydrate_ui_from_settings, seed_initial_view_model, spawn_initial_albums_fetch,
    spawn_initial_artists_fetch, spawn_initial_genres_fetch, spawn_initial_playlists_fetch,
    spawn_initial_tracks_fetch,
};
pub use subscribers::{
    install_audio_device_lost_subscriber, install_library_changed_refresher,
    install_rescan_notice_subscriber, install_toast_bridge,
};
pub use views::{install_library_settings_and_friends, install_views};

#[cfg(test)]
#[path = "../tests/ui_setup_tests.rs"]
mod tests;
