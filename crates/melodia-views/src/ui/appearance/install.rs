//! The big startup `install` function. Hydrates the Slint `Settings`
//! global from `settings.json`, applies the resolved palette, wires
//! every chip callback, and seeds the Material You repaint subscriber.

use std::sync::Arc;

use async_compat::Compat;
use parking_lot::RwLock;
use slint::ComponentHandle;
use tokio::sync::watch;

use super::repaint::repaint_from_settings;
use super::{
    AppearanceHandles, PersistedAccent, accent_picker, apply_and_seed, material_you_sync,
    read_initial_system_state, seed_theme_names, system_watcher, theme_picker, window_settings,
};
use melodia_app::library;
use melodia_app::services;
use melodia_app::state::{AppState, Signal};
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, Settings, Theme};

/// Hydrate the Settings global from `settings.json`, paint the resolved
/// palette, and wire chip-click callbacks. Call once during startup,
/// before `hydrate_ui_from_settings` so the UI's first frame uses the
/// correct theme. Returns handles `main.rs` forwards to
/// `tasks::material_you` so the coordinator can write dynamic palettes
/// back into `os_state` and kick repaints.
pub fn install(ui: &AppWindow, state: &AppState) -> Result<AppearanceHandles, AppError> {
    let settings = library::settings::get_settings(state)?;

    // Read the OS appearance state synchronously. We're on the main
    // thread, before the Slint event loop, so a brief D-Bus + file read
    // is the cheapest option — no async plumbing needed for a one-shot.
    let initial_state = read_initial_system_state();
    let os_state = Arc::new(RwLock::new(initial_state.clone()));

    let kick = Signal::new();

    // Repaint channel — the Material You coordinator writes the latest
    // `SystemColorState` snapshot here after each palette generation;
    // the subscriber below applies it on the UI thread.
    let (repaint_tx, mut repaint_rx) = watch::channel(initial_state.clone());
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let res = slint::spawn_local(Compat::new(async move {
            while repaint_rx.changed().await.is_ok() {
                let snap = repaint_rx.borrow_and_update().clone();
                let Some(ui) = weak.upgrade() else { return };
                repaint_from_settings(&ui, &state, &snap);
            }
        }));
        if let Err(e) = res {
            log::warn!("material_you repaint subscriber: {e}");
        }
    }

    // Spawn the watcher (fans portal `SettingChanged` signals into a
    // `watch` channel) and a UI-thread consumer that repaints whenever
    // the persisted variant is `"system"`.
    system_watcher::spawn_os_state_watcher(
        ui,
        state,
        os_state.clone(),
        initial_state.clone(),
        kick.clone(),
    );

    seed_theme_names(ui);
    let initial_last_static = settings
        .theme_preferences
        .get(&settings.theme_id)
        .and_then(|p| p.last_static_accent.clone());
    apply_and_seed(
        ui,
        &settings.theme_id,
        &settings.theme_variant,
        &settings.accent_color,
        &settings.dynamic_color_style,
        initial_last_static.as_deref(),
        &initial_state,
    );

    let persisted_accent: PersistedAccent =
        Arc::new(parking_lot::Mutex::new(settings.accent_color.clone()));

    // Seed the Match Unfocused Window Background row.
    {
        let g = ui.global::<Settings>();
        g.set_match_unfocused_supported(services::settings::is_kde_desktop());
        g.set_match_unfocused_bg(settings.layout.match_unfocused_to_system_bg);
    }

    // Seed Window Corner Radius. `Settings.corner-radius` is the UI's
    // single source of truth (chips bind to it); `Theme.shell-radius`
    // drives the actual painted rounding on the outer mantle shell +
    // inner content panel in custom-titlebar mode. Both must be set
    // before `app.run()` so the first frame renders with the persisted
    // radius. The persisted value is clamped *and* snapped to the
    // nearest chip preset before seeding.
    #[allow(
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        reason = "corner_radius clamped to 0..=15: exact in i32 and exact in f32 (f32 mantissa has 23 bits)"
    )]
    {
        let clamped = settings.corner_radius.min(services::settings::MAX_CORNER_RADIUS);
        let radius = library::settings::snap_to_preset(clamped);
        ui.global::<Settings>().set_corner_radius(radius as i32);
        let theme = ui.global::<Theme>();
        theme.set_shell_radius(radius as f32);
        theme.set_native_content_radius(services::settings::get_os_corner_radius() as f32);
    }

    // Seed the Decoration Button Style + Side rows. Same shape as the
    // corner-radius seed above: `Settings.*` is the UI source of truth
    // (chip selection), `Theme.*` drives `custom-titlebar.slint`'s
    // conditional layout. Both written before `app.run()` so the first
    // painted frame reflects the persisted choice.
    {
        let style_idx = window_settings::idx_for(settings.window.titlebar_button_style);
        let side_idx = window_settings::idx_for_side(settings.window.titlebar_button_side);
        let g = ui.global::<Settings>();
        g.set_titlebar_button_style(style_idx);
        g.set_titlebar_button_side(side_idx);
        let theme = ui.global::<Theme>();
        theme.set_titlebar_button_style(style_idx);
        theme.set_titlebar_button_side(side_idx);
    }

    // Seed the tray toggles. `tray_enabled` drives the "System Tray Icon"
    // master switch. `close_to_tray` is mirrored into both the Slint global
    // (drives its switch) and the `tray_bridge` atomic (drives
    // `window_chrome`'s close handlers) so a close-to-tray-enabled launch
    // behaves correctly before the user ever opens Settings.
    {
        ui.global::<Settings>().set_tray_enabled(settings.tray.tray_enabled);
        let on = settings.tray.close_to_tray;
        ui.global::<Settings>().set_close_to_tray(on);
        crate::ui::shell::tray_bridge::set_close_to_tray(on);
    }

    // Seed the Overflow Menu Buttons checkboxes from
    // `settings.overflow_buttons`. The Vec<String> stores ids of buttons
    // that have been MOVED INTO the overflow popup, so the boolean is
    // `true` iff the id is present.
    {
        let g = ui.global::<Settings>();
        let v = &settings.overflow_buttons;
        g.set_overflow_favorite(v.iter().any(|x| x == "favorite"));
        g.set_overflow_repeat(v.iter().any(|x| x == "repeat"));
        g.set_overflow_shuffle(v.iter().any(|x| x == "shuffle"));
        g.set_overflow_pin(v.iter().any(|x| x == "pin"));
        g.set_overflow_queue(v.iter().any(|x| x == "queue"));
    }

    // The boot migration for a `settings.json` written before `theme_preferences`
    // existed; it takes the settings read above rather than re-reading them.
    if let Err(e) = library::settings::seed_theme_preference(state, settings) {
        log::warn!("seed theme_preferences: {e}");
    }

    theme_picker::wire_theme_changed(
        ui,
        state,
        os_state.clone(),
        kick.clone(),
        persisted_accent.clone(),
    );
    theme_picker::wire_variant_changed(
        ui,
        state,
        os_state.clone(),
        kick.clone(),
        persisted_accent.clone(),
    );
    accent_picker::wire_accent_changed(
        ui,
        state,
        os_state.clone(),
        kick.clone(),
        persisted_accent.clone(),
    );
    material_you_sync::wire_color_style_changed(
        ui,
        state,
        os_state.clone(),
        kick.clone(),
        persisted_accent,
    );
    window_settings::wire_match_unfocused_bg_changed(ui, state);
    window_settings::wire_corner_radius_changed(ui, state);
    window_settings::wire_titlebar_button_style_changed(ui, state);
    window_settings::wire_titlebar_button_side_changed(ui, state);
    window_settings::wire_overflow_buttons_changed(ui, state);
    window_settings::wire_close_to_tray_changed(ui, state);

    Ok(AppearanceHandles {
        os_state,
        kick,
        repaint_tx,
    })
}
