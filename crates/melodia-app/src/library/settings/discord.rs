//! Discord Rich Presence setters. These persist the enable + sub-option toggles
//! to `settings.json`; the in-memory shadow on `DiscordPresenceService` is
//! refreshed separately by the UI callback via `set_flags` once the write
//! commits, the same kick-after-persist ordering [`super::scrobble`] uses.
//! Discord has no *credentials* — the application id is a compile-time
//! constant — so nothing lives outside `settings.json`.

use crate::services;
use crate::state::AppState;
use melodia_core::error::AppError;

/// Persist the Discord Rich Presence master toggle. Off by default (opt-in).
pub fn set_discord_rpc_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.discord.discord_rpc_enabled = enabled;
    })
}

/// Persist the album-artwork toggle. On by default, but inert until the parent
/// toggle is enabled — it drives the outbound cover lookup.
pub fn set_discord_rpc_artwork(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.discord.discord_rpc_artwork = enabled;
    })
}

/// Persist the hide-while-paused toggle. Off by default (the card stays up with
/// a paused marker).
pub fn set_discord_rpc_hide_when_paused(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.discord.discord_rpc_hide_when_paused = enabled;
    })
}
