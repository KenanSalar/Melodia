//! The Discord Rich Presence card. No credentials or login, the application id being compiled
//! in, so only the status side: seeded from `DiscordPresenceService::status()` and kept current
//! by a `watch<DiscordStatus>` subscriber, so a Discord restart flips the row live.

use async_compat::Compat;
use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::config::Paths;
use melodia_core::entities::integrations::DiscordFlags;
use melodia_core::error::AppError;
use melodia_integrations::services::integrations::discord::DiscordStatus;
use melodia_ui::{AppWindow, Settings};

/// The one writer of the connection and enabled rows, for the seed and every status change.
fn paint_status(ui: &AppWindow, status: DiscordStatus) {
    let g = ui.global::<Settings>();
    g.set_discord_rpc_connected(status.connected);
    g.set_discord_rpc_enabled(status.enabled);
}

/// One toggle's handler: the field flipped in a fresh flags snapshot and applied synchronously,
/// so the detector and the worker see it at once (`set_flags` starts and stops the worker), then
/// persisted.
///
/// `nudge_detector`, the master toggle's, forces one no-op view-model emit: the detector reacts
/// only to that watch, so enabling mid-playback would otherwise paint nothing until the player
/// next moved. The enable edge resets its dedupe, so the republish paints.
fn discord_toggle_binding(
    state: &AppState,
    label: &'static str,
    nudge_detector: bool,
    set_field: fn(&mut DiscordFlags, bool),
    persist: fn(&Paths, bool) -> Result<(), AppError>,
) -> impl FnMut(bool) + 'static {
    let state = state.clone();
    move |on| {
        let mut flags = state.discord.flags();
        set_field(&mut flags, on);
        state.discord.set_flags(flags);
        if nudge_detector {
            melodia_engine::player::engine::state::with_state_emit(
                &state.player_state,
                &state.sinks,
                |_: &mut melodia_engine::player::engine::state::PlayerState| {},
            );
        }
        state.persist_blocking(label, move |paths| persist(paths, on));
    }
}

pub fn install_discord(ui: &AppWindow, state: &AppState) {
    // Seeded once: not part of `DiscordStatus`, so the watch never touches them.
    {
        let flags = state.discord.flags();
        let g = ui.global::<Settings>();
        g.set_discord_rpc_artwork(flags.discord_rpc_artwork);
        g.set_discord_rpc_hide_when_paused(flags.discord_rpc_hide_when_paused);
    }

    let mut rx = state.discord.subscribe_status();
    paint_status(ui, state.discord.status());
    {
        let weak = ui.as_weak();
        let _ = slint::spawn_local(Compat::new(async move {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let snapshot = *rx.borrow_and_update();
                let Some(ui) = weak.upgrade() else { break };
                paint_status(&ui, snapshot);
            }
        }));
    }

    let g = ui.global::<Settings>();
    g.on_discord_rpc_enabled_changed(discord_toggle_binding(
        state,
        "set_discord_rpc_enabled",
        true,
        |f, on| f.discord_rpc_enabled = on,
        library::settings::set_discord_rpc_enabled,
    ));
    g.on_discord_rpc_artwork_changed(discord_toggle_binding(
        state,
        "set_discord_rpc_artwork",
        false,
        |f, on| f.discord_rpc_artwork = on,
        library::settings::set_discord_rpc_artwork,
    ));
    g.on_discord_rpc_hide_when_paused_changed(discord_toggle_binding(
        state,
        "set_discord_rpc_hide_when_paused",
        false,
        |f, on| f.discord_rpc_hide_when_paused = on,
        library::settings::set_discord_rpc_hide_when_paused,
    ));
}
