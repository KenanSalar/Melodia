//! Wire the Discord Rich Presence settings section to Rust.
//!
//! Simpler than [`super::scrobbling_settings`] — Discord has no credentials
//! or login dialog (the application id is a compile-time constant), so this is
//! only the status side. The connection/enabled props are sync-seeded from
//! `DiscordPresenceService::status()` (like [`crate::ui::replaygain`]) and then
//! kept current by a `watch<DiscordStatus>` subscriber — the single writer of
//! `Settings.discord-rpc-connected`, so a Discord restart flips the status row
//! live without reopening the section. The three toggles flip a field in a fresh
//! `DiscordFlags` snapshot, apply it to the shadow via `set_flags` (which
//! starts/stops the IPC worker), then persist.

use async_compat::Compat;
use slint::ComponentHandle;

use crate::error::AppError;
use crate::library;
use crate::services::discord::DiscordStatus;
use crate::services::settings::DiscordFlags;
use crate::state::AppState;
use crate::{AppWindow, Settings};

/// Paint the connection/enabled props from a status snapshot. The single writer
/// of these props — called once for the seed and then on every `subscribe_status`
/// change (e.g. a background Discord restart flipping `connected`).
fn paint_status(ui: &AppWindow, status: DiscordStatus) {
    let g = ui.global::<Settings>();
    g.set_discord_rpc_connected(status.connected);
    g.set_discord_rpc_enabled(status.enabled);
}

/// Build the change-handler for one Discord toggle: flip that field in a fresh
/// flags snapshot, apply it to the shadow synchronously (so the detector task and
/// worker see it at once — `set_flags` also starts/stops the worker), then
/// persist. Sibling of `scrobbling_settings::scrobble_toggle_binding`, minus the
/// `on_enable` side-effect (Discord's worker lifecycle lives inside `set_flags`).
///
/// `nudge_detector` (only the master toggle) forces one no-op view-model emit
/// after `set_flags`, so enabling mid-playback paints the card at once instead of
/// waiting for the next player-state change — the detector reacts only to the
/// `view_model` watch, which a settings toggle doesn't otherwise disturb. The
/// disabled→enabled edge resets the detector's dedupe, so this republish paints.
/// Mirrors the Windows SMTC-attach flush in `main.rs`.
fn discord_toggle_binding(
    state: &AppState,
    label: &'static str,
    nudge_detector: bool,
    set_field: fn(&mut DiscordFlags, bool),
    persist: fn(&AppState, bool) -> Result<(), AppError>,
) -> impl FnMut(bool) + 'static {
    let state = state.clone();
    move |on| {
        let mut flags = state.discord.flags();
        set_field(&mut flags, on);
        state.discord.set_flags(flags);
        if nudge_detector {
            crate::player::state::with_state_emit(
                &state.player_state,
                &state.sinks,
                |_: &mut crate::player::state::PlayerState| {},
            );
        }
        state.persist_blocking(label, move |s| persist(s, on));
    }
}

pub fn install_discord(ui: &AppWindow, state: &AppState) {
    // Seed the sub-option flags once. They aren't part of `DiscordStatus`, so the
    // status watch never touches them — they're user-owned after this seed.
    {
        let flags = state.discord.flags();
        let g = ui.global::<Settings>();
        g.set_discord_rpc_artwork(flags.discord_rpc_artwork);
        g.set_discord_rpc_hide_when_paused(flags.discord_rpc_hide_when_paused);
    }

    // Seed the connection status, then keep it current from the watch.
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
