//! The appearance cards' boot: their rows seeded, the palette painted, every picker wired.

use std::sync::Arc;

use async_compat::Compat;
use parking_lot::RwLock;
use slint::ComponentHandle;
use tokio::sync::watch;

use super::repaint::{apply_settings, repaint_from_settings};
use super::{
    AppearanceHandles, PersistedAccent, accent_picker, material_you_sync,
    read_initial_system_state, seed_theme_registry, system_watcher, theme_picker, window_border,
    window_settings,
};
use melodia_app::library;
use melodia_app::services::settings::SettingsData;
use melodia_app::state::{AppState, Signal};
use melodia_core::error::{AppError, describe};
use melodia_core::themes::SystemColorState;
use melodia_platform::services::platform::desktop;
use melodia_ui::{AppWindow, MiniPlayer, Settings, Theme};

/// Seeds the cards, paints the palette and wires the pickers, ahead of `hydrate_ui_from_settings`
/// so the first frame paints the persisted theme.
pub fn install(ui: &AppWindow, state: &AppState) -> Result<AppearanceHandles, AppError> {
    let settings = library::settings::get_settings(&state.paths)?;

    // Synchronously: ahead of the event loop, one D-Bus and file read wants no async plumbing.
    let initial_state = read_initial_system_state();
    let os_state = Arc::new(RwLock::new(initial_state.clone()));

    let kick = Signal::new();
    let repaint_tx = spawn_repaint_subscriber(ui, state, initial_state.clone());

    system_watcher::watch_os_state(
        ui,
        state,
        os_state.clone(),
        initial_state.clone(),
        kick.clone(),
    );

    seed_theme_registry(ui);
    // Ahead of `apply_settings`, whose palette apply resolves the border colour seeded here.
    window_border::seed(ui, &settings.window);
    apply_settings(ui, &settings, &initial_state);
    seed_window_rows(ui, &settings);

    // The boot migration for a `settings.json` written before `theme_preferences`
    // existed.
    if let Err(e) = library::settings::seed_theme_preference(&state.paths) {
        log::warn!("seed theme_preferences: {}", describe(&e));
    }

    let persisted_accent: PersistedAccent =
        Arc::new(parking_lot::Mutex::new(settings.accent_color.clone()));
    wire_pickers(ui, state, &os_state, &kick, &persisted_accent);
    wire_window_rows(ui, state);

    Ok(AppearanceHandles { os_state, kick, repaint_tx })
}

fn spawn_repaint_subscriber(
    ui: &AppWindow,
    state: &AppState,
    initial_state: SystemColorState,
) -> watch::Sender<SystemColorState> {
    let (repaint_tx, mut repaint_rx) = watch::channel(initial_state);
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
        log::warn!("material_you repaint subscriber: {}", describe(&e));
    }
    repaint_tx
}

/// The window rows' persisted values, written before `app.show()` so the first
/// painted frame reflects them.
fn seed_window_rows(ui: &AppWindow, settings: &SettingsData) {
    {
        let g = ui.global::<Settings>();
        g.set_match_unfocused_supported(desktop::is_kde_desktop());
        g.set_match_unfocused_bg(settings.layout.match_unfocused_to_system_bg);
    }

    // The chips read `Settings.corner-radius`, the shell and the content panel paint
    // `Theme.shell-radius`.
    #[allow(
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        reason = "corner_radius clamped to 0..=15: exact in i32 and exact in f32 (f32 mantissa has 23 bits)"
    )]
    {
        let radius = window_settings::corner_radius_preset(settings.corner_radius);
        ui.global::<Settings>().set_corner_radius(radius as i32);
        let theme = ui.global::<Theme>();
        theme.set_shell_radius(radius as f32);
        theme.set_native_content_radius(desktop::get_os_corner_radius() as f32);
    }

    // Likewise the chips read `Settings`, and `custom-titlebar.slint` lays out off `Theme`.
    {
        let style_idx = window_settings::idx_for(settings.window.titlebar_button_style);
        let side_idx = window_settings::idx_for_side(settings.window.titlebar_button_side);
        let mini_style_idx = window_settings::idx_for(settings.window.mini_player_button_style);
        let g = ui.global::<Settings>();
        g.set_titlebar_button_style(style_idx);
        g.set_titlebar_button_side(side_idx);
        g.set_mini_player_button_style(mini_style_idx);
        let theme = ui.global::<Theme>();
        theme.set_titlebar_button_style(style_idx);
        theme.set_titlebar_button_side(side_idx);
        ui.global::<MiniPlayer>().set_button_style(mini_style_idx);
    }

    // `close_to_tray` goes into the `tray_bridge` atomic too, which the close handlers read
    // whether or not Settings was ever opened.
    {
        ui.global::<Settings>().set_tray_enabled(settings.tray.tray_enabled);
        let on = settings.tray.close_to_tray;
        ui.global::<Settings>().set_close_to_tray(on);
        crate::ui::shell::tray_bridge::set_close_to_tray(on);
    }

    // `overflow_buttons` lists the buttons moved into the overflow menu.
    {
        let g = ui.global::<Settings>();
        let v = &settings.overflow_buttons;
        g.set_overflow_favorite(v.iter().any(|x| x == "favorite"));
        g.set_overflow_repeat(v.iter().any(|x| x == "repeat"));
        g.set_overflow_shuffle(v.iter().any(|x| x == "shuffle"));
        g.set_overflow_pin(v.iter().any(|x| x == "pin"));
        g.set_overflow_queue(v.iter().any(|x| x == "queue"));
        g.set_overflow_quality(v.iter().any(|x| x == "quality"));
    }
}

/// The theme, variant, accent and colour-style pickers.
fn wire_pickers(
    ui: &AppWindow,
    state: &AppState,
    os_state: &Arc<RwLock<SystemColorState>>,
    kick: &Signal,
    persisted_accent: &PersistedAccent,
) {
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
    accent_picker::wire_accent_changed(ui, state, os_state.clone(), persisted_accent.clone());
    accent_picker::wire_follows_artwork_changed(
        ui,
        state,
        os_state.clone(),
        persisted_accent.clone(),
    );
    material_you_sync::wire_color_style_changed(ui, state, kick.clone(), persisted_accent.clone());
}

fn wire_window_rows(ui: &AppWindow, state: &AppState) {
    window_settings::wire_match_unfocused_bg_changed(ui, state);
    window_settings::wire_corner_radius_changed(ui, state);
    window_settings::wire_titlebar_button_style_changed(ui, state);
    window_settings::wire_titlebar_button_side_changed(ui, state);
    window_settings::wire_mini_player_button_style_changed(ui, state);
    window_settings::wire_overflow_buttons_changed(ui, state);
    window_settings::wire_close_to_tray_changed(ui, state);
    window_border::wire(ui, state);
}
