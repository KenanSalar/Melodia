//! Everything the window needs before `app.show()`, in the order it has to happen.

use std::rc::Rc;
use std::sync::Arc;

use melodia_app::services::settings::SettingsData;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_app::{services, tasks};
use melodia_core::error::{AppError, AppResult, describe};
use melodia_platform::services::platform;
use melodia_ui::AppWindow;
use melodia_views::ui;
use melodia_views::ui::shell::notifications::NotificationsUi;
use slint::ComponentHandle;
use tokio::sync::watch;

use super::chrome::{
    apply_backdrop_style, install_app_chrome, install_backdrop_dither, install_locale,
};
use super::hydrate::{
    hydrate_ui_from_settings, seed_initial_view_model, spawn_initial_albums_fetch,
    spawn_initial_artists_fetch, spawn_initial_genres_fetch, spawn_initial_playlists_fetch,
    spawn_initial_tracks_fetch,
};
use super::subscribers::{
    install_artwork_restore_subscriber, install_audio_device_lost_subscriber,
    install_library_changed_refresher, install_rescan_notice_subscriber, install_rss_sampler,
    install_toast_bridge,
};
use super::views::{UiHandles, install_library_settings_and_friends, install_views};
use crate::boot::tasks::serve_file_opens;

/// Installs every surface, subscriber and boot-time task the window carries. Each closure holds
/// its own clone of what it reads, so nothing here outlives the call.
pub fn install_ui(
    app: &AppWindow,
    state: &AppState,
    spawner: &tasks::TaskSpawner,
    settings: Option<&SettingsData>,
    view_state: Option<&ViewStateData>,
    file_open_listener: Option<platform::single_instance::Listener>,
) -> AppResult<()> {
    let (views, notifications) = install_shell(app, state, spawner, settings, view_state)?;
    install_player_surfaces(app, state, &views)?;
    seed_and_fetch(app, state, &views, view_state);
    install_notices(app, state, &views, &notifications)?;
    install_updater_and_onboarding(app, state, spawner, settings, &notifications);
    install_platform_hooks(app, state, spawner, settings, file_open_listener);
    Ok(())
}

/// The window's chrome, every view, the Settings sections and the appearance, then the
/// persisted state hydrated over them.
fn install_shell(
    app: &AppWindow,
    state: &AppState,
    spawner: &tasks::TaskSpawner,
    settings: Option<&SettingsData>,
    view_state: Option<&ViewStateData>,
) -> AppResult<(UiHandles, Rc<NotificationsUi>)> {
    install_locale(app, state, settings);
    install_app_chrome(app, state);
    // Ahead of `install_views`, which builds the artwork tiers this decides the shape of.
    if apply_backdrop_style(app, settings) {
        install_backdrop_dither(app);
    }
    let views = install_views(app, state, view_state);
    let notifications = install_library_settings_and_friends(app, state)?;

    // These three wire here rather than inside their slices because their
    // completion toasts need the `Rc<NotificationsUi>`.
    ui::playlists::wire_files(app, state, &views.playlists_ui, &notifications);
    ui::radio::wire_files(app, state, &views.radio_ui, &notifications);
    ui::callbacks::wire_tags(app, state, &notifications);

    match ui::appearance::install(app, state) {
        Ok(h) => tasks::material_you::spawn(
            spawner,
            state.clone(),
            h.os_state.clone(),
            state.sinks.view_model.subscribe(),
            h.kick.subscribe(),
            h.repaint_tx.clone(),
            views.cover_thumbs.clone(),
        ),
        Err(e) => log::warn!("appearance::install: {}", describe(&e)),
    }

    hydrate_ui_from_settings(app, state, settings, view_state);
    Ok((views, notifications))
}

/// The player's subscribers and the surfaces that render it: the queue panel, Now Playing and the
/// miniplayer.
fn install_player_surfaces(app: &AppWindow, state: &AppState, views: &UiHandles) -> AppResult<()> {
    let weak = app.as_weak();
    let shown = ui::shell::bridge::ShownPosition::default();
    ui::shell::bridge::spawn_view_model_subscriber(
        weak.clone(),
        &state.sinks,
        views.cover_thumbs.clone(),
        state.runtime.clone(),
        shown.clone(),
    )
    .map_err(|e| AppError::io("view-model subscriber", e))?;
    ui::shell::bridge::spawn_queue_subscriber(weak.clone(), &state.sinks)
        .map_err(|e| AppError::io("queue subscriber", e))?;
    ui::shell::bridge::spawn_position_subscriber(weak.clone(), &state.position_tx, shown)
        .map_err(|e| AppError::io("position subscriber", e))?;
    ui::track_links::spawn_playing_links_subscriber(weak.clone(), state)
        .map_err(|e| AppError::io("playing-track links subscriber", e))?;
    // Here rather than in `radio::install`, so every player-channel subscriber is spawned in one
    // place.
    ui::radio::install_history(weak, &views.radio_ui, &state.sinks)
        .map_err(|e| AppError::io("station history subscriber", e))?;

    if let Err(e) = ui::queue_panel::install(app, state, &views.cover_thumbs) {
        log::warn!("queue_panel::install: {}", describe(&e));
    }

    let np_artwork = Arc::new(ui::now_playing_artwork::NowPlayingArtwork::new(
        ui::now_playing_artwork::blur_spec(app),
    ));
    match ui::now_playing::install(app, state, &views.cover_thumbs, &np_artwork) {
        // The miniplayer takes `np_state` for its up-next gate.
        Ok(np_state) => {
            if let Err(e) = ui::shell::mini_player::install(app, state, &np_state) {
                log::warn!("mini_player::install: {}", describe(&e));
            }
        }
        Err(e) => log::warn!("now_playing::install: {}", describe(&e)),
    }
    Ok(())
}

fn seed_and_fetch(
    app: &AppWindow,
    state: &AppState,
    views: &UiHandles,
    view_state: Option<&ViewStateData>,
) {
    seed_initial_view_model(app, state, &views.cover_thumbs);

    spawn_initial_tracks_fetch(state, &views.tracks_ui, view_state);
    spawn_initial_albums_fetch(app, state, &views.albums_ui);
    spawn_initial_artists_fetch(app, state, &views.artists_ui);
    spawn_initial_genres_fetch(app, state, &views.genres_ui);
    spawn_initial_playlists_fetch(app, state, &views.playlists_ui);
}

/// The subscribers that raise a toast or refresh a surface on a backend event.
fn install_notices(
    app: &AppWindow,
    state: &AppState,
    views: &UiHandles,
    notifications: &Rc<NotificationsUi>,
) -> AppResult<()> {
    let weak = app.as_weak();
    install_library_changed_refresher(state, &views.tracks_ui, weak.clone())?;
    install_rescan_notice_subscriber(state, weak.clone(), notifications.clone())?;
    install_audio_device_lost_subscriber(state, weak.clone(), notifications.clone())?;
    install_artwork_restore_subscriber(state, weak.clone(), notifications.clone())?;

    ui::support::install(app, state, notifications.clone())?;

    install_toast_bridge(weak.clone(), notifications.clone())?;

    install_rss_sampler(weak)
}

fn install_updater_and_onboarding(
    app: &AppWindow,
    state: &AppState,
    spawner: &tasks::TaskSpawner,
    settings: Option<&SettingsData>,
    notifications: &Rc<NotificationsUi>,
) {
    let weak = app.as_weak();
    // One `watch` carries the daily task's and the `Updater.*` callbacks' events to the
    // subscriber that toasts them.
    let (updater_event_tx, updater_event_rx) =
        watch::channel::<Option<services::updater::UpdaterEvent>>(None);
    ui::settings::updater_settings::install_event_subscriber(
        weak.clone(),
        notifications.clone(),
        updater_event_rx,
    );
    ui::callbacks::wire_updater(app, state, notifications, &updater_event_tx);

    // After `hydrate_ui_from_settings`, so a deep link out of the card doesn't race the boot's own
    // nav and tab writes. The crash notice and the update check wait in this closure until the
    // card closes rather than stacking on it; a launch with no card runs it at once.
    let deferred_spawner = spawner.clone();
    let deferred_state = state.clone();
    let deferred_notifications = notifications.clone();
    ui::onboarding::install(app, state, settings, move || {
        if let Some(ui) = weak.upgrade() {
            ui::settings::diagnostics::notify_previous_crash(
                &ui,
                &deferred_state,
                &deferred_notifications,
            );
        }

        // Gated only on the install's shape, which can't change while running. The auto-check
        // switch is read per tick, the welcome card offering it after this point.
        if services::updater::is_available() && !platform::install_kind::is_system_install() {
            tasks::updater_daily::spawn(
                &deferred_spawner,
                deferred_state.clone(),
                ui::callbacks::check_painter(weak.clone()),
                updater_event_tx,
            );
        } else {
            log::info!(
                "updater_daily: not spawning (available={}, system_managed={})",
                services::updater::is_available(),
                platform::install_kind::is_system_install()
            );
        }
    });
}

/// The boot-time housekeeping, the tray, the forwarded-launch server, and the Windows steps that
/// need the window's handle.
fn install_platform_hooks(
    app: &AppWindow,
    state: &AppState,
    spawner: &tasks::TaskSpawner,
    settings: Option<&SettingsData>,
    file_open_listener: Option<platform::single_instance::Listener>,
) {
    // Independent of the daily check, or a cancelled install's package stays staged until the
    // next install click. The delay matches `updater_daily::STARTUP_DELAY`, leaving the launch
    // scan the disk first.
    state.runtime.spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        services::updater::prune_stale_staging().await;
    });

    #[cfg(target_os = "linux")]
    state.runtime.spawn_blocking(|| {
        if let Err(e) = platform::desktop_integration::refresh_user_install() {
            log::warn!("desktop_integration: refresh failed: {}", describe(&e));
        }
    });

    // The one gate, the setting being restart-gated: off, there is no tray connection, thread or
    // task at all.
    if settings.is_some_and(|s| s.tray.tray_enabled) {
        ui::shell::tray_bridge::install(spawner, state, app);
    }

    // Here rather than beside the boot's claim: a forwarded launch raises the window.
    if let Some(listener) = file_open_listener {
        serve_file_opens(state, app, listener);
    }

    #[cfg(target_os = "windows")]
    {
        attach_media_controls_after_show(app, state);
        polish_titlebar_after_show(app);
    }
}

/// Attaches SMTC on the loop's first pass, once the window is shown: it needs a real `HWND`, and
/// souvlaki panics on a null one.
#[cfg(target_os = "windows")]
fn attach_media_controls_after_show(app: &AppWindow, state: &AppState) {
    let Some(mc) = state.media_controls.clone() else { return };
    let weak = app.as_weak();
    let player_state = state.player_state.clone();
    let sinks = state.sinks.clone();
    if let Err(e) = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        match ui::window_chrome::win32_hwnd(&app) {
            Some(hwnd) => {
                if mc.attach_smtc(hwnd) {
                    // `sync()` skipped every change while inert, leaving the OS panel empty.
                    melodia_engine::player::engine::state::with_state_emit(
                        &player_state,
                        &sinks,
                        |_| {},
                    );
                }
            }
            None => {
                log::warn!("Win32 HWND unavailable after window show; SMTC disabled");
            }
        }
    }) {
        log::warn!("Failed to schedule Windows SMTC attach: {}", describe(&e));
    }
}

/// Repeats the DWM caption paint the boot's theme apply made with no `HWND` yet, and sets the
/// caption icon. Every later palette change reaches DWM through `theme_paint`.
#[cfg(target_os = "windows")]
fn polish_titlebar_after_show(app: &AppWindow) {
    let weak = app.as_weak();
    if let Err(e) = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        install_window_icon(&app);
        ui::appearance::theme_paint::reapply_from_theme(&app);
    }) {
        log::warn!("Failed to schedule Windows titlebar polish: {}", describe(&e));
    }
}

/// Puts the EXE's embedded icon on the caption, whose WNDCLASS winit registers with `hIcon: 0`;
/// the taskbar reads the EXE itself.
#[cfg(target_os = "windows")]
fn install_window_icon(app: &AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::platform::windows::IconExtWindows;
    use slint::winit_030::winit::window::Icon;

    match Icon::from_resource(1, None) {
        Ok(icon) => {
            app.window().with_winit_window(|w| {
                w.set_window_icon(Some(icon));
            });
        }
        Err(e) => {
            log::warn!(
                "Failed to load Melodia icon from EXE resource (ordinal 1): {}",
                describe(&e)
            );
        }
    }
}
