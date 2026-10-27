//! The services that need the database or the app state: settings, the two JSON state files,
//! artist images, the diagnostics bundle and the updater.

pub mod artist_images;
pub mod diagnostics;
pub mod search_history;
pub mod settings;
pub mod updater;
pub mod view_state;
