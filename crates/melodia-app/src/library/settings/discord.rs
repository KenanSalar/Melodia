//! Discord Rich Presence setters. There are no credentials, the application id being a
//! compile-time constant, so nothing lives outside `settings.json`.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

pub fn set_discord_rpc_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.discord.discord_rpc_enabled = enabled;
    })
}

/// Drives the outbound cover lookup.
pub fn set_discord_rpc_artwork(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.discord.discord_rpc_artwork = enabled;
    })
}

/// Off leaves the card up with a paused marker.
pub fn set_discord_rpc_hide_when_paused(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.discord.discord_rpc_hide_when_paused = enabled;
    })
}
