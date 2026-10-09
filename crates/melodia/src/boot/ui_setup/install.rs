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

/// Install every surface, subscriber and boot-time task the window carries.
///
/// Nothing here has to outlive the call in `main`: every installed closure and subscriber holds
/// its own clone of what it reads.
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
        // After `appearance::install`, whose kick channel it subscribes to.
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

/// The player's subscribers and the surfaces that render it: the queue sheet, Now Playing and the
/// miniplayer.
fn install_player_surfaces(app: &AppWindow, state: &AppState, views: &UiHandles) -> AppResult<()> {
    let weak = app.as_weak();
    ui::shell::bridge::spawn_view_model_subscriber(
        weak.clone(),
        &state.sinks,
        views.cover_thumbs.clone(),
        state.runtime.clone(),
    )
    .map_err(|e| AppError::io("view-model subscriber", e))?;
    ui::shell::bridge::spawn_queue_subscriber(weak.clone(), &state.sinks)
        .map_err(|e| AppError::io("queue subscriber", e))?;
    ui::shell::bridge::spawn_position_subscriber(weak.clone(), &state.position_tx)
        .map_err(|e| AppError::io("position subscriber", e))?;
    // Reads the same view model as the first of those, for the ICY titles a station announces.
    // Beside them rather than inside `radio::install`, so every subscription to a player channel
    // is spawned in one place.
    ui::radio::install_history(weak, &views.radio_ui, &state.sinks)
        .map_err(|e| AppError::io("station history subscriber", e))?;

    match ui::queue_sheet::install(app, state, &views.cover_thumbs) {
        Ok(h) => ui::window_chrome::set_queue_sheet_open(h.is_open),
        Err(e) => log::warn!("queue_sheet::install: {}", describe(&e)),
    }

    // Now Playing owns its own small `(cover, blur)` tier, separate from
    // `cover_thumbs`.
    let np_artwork = Arc::new(ui::now_playing_artwork::NowPlayingArtwork::new(
        ui::now_playing_artwork::blur_spec(app),
    ));
    match ui::now_playing::install(app, state, &views.cover_thumbs, &np_artwork) {
        // Needs `np_state` for the up-next subscriber gate; without it the gate
        // would flip nothing visible.
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
    let weak = app.as_weak();
    seed_initial_view_model(app, state, &views.cover_thumbs);

    spawn_initial_tracks_fetch(state, &views.tracks_ui, view_state, weak.clone());
    spawn_initial_albums_fetch(state, &views.albums_ui, weak.clone());
    spawn_initial_artists_fetch(state, &views.artists_ui, weak.clone());
    spawn_initial_genres_fetch(state, &views.genres_ui, weak.clone());
    spawn_initial_playlists_fetch(state, &views.playlists_ui, weak);
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

    // The Ko-fi link, plus the one-time support toast a few minutes into
    // whichever early launch is the fifth. Counts this launch either way.
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
    // One `watch` carries backend events from the daily task and the `Updater.*`
    // callbacks to the UI-thread subscriber that toasts them. The daily task
    // spawns only where the install path is user-writable; system-managed
    // installs go through the OS package manager instead.
    let (updater_event_tx, updater_event_rx) =
        watch::channel::<Option<services::updater::UpdaterEvent>>(None);
    ui::settings::updater_settings::install_event_subscriber(
        weak.clone(),
        notifications.clone(),
        updater_event_rx,
    );
    ui::callbacks::wire_updater(app, state, notifications, &updater_event_tx);

    // The first-run card, opened only if this install hasn't seen the current revision. After
    // `hydrate_ui_from_settings`, so a deep link out of it isn't racing the boot's own nav and tab
    // writes, and before `app.show()`, like everything else that seeds a Slint property.
    //
    // The two surfaces that would otherwise land on top of it wait in this closure: the first
    // thirty seconds shouldn't be three stacked surfaces. Held rather than skipped — the crash
    // notice consumes its marker as it fires — and it runs immediately on every launch that opens
    // no card. Deferring a *daily* check by the length of a welcome card costs nothing. The Ko-fi
    // prompt can't collide, being spent on the fifth launch behind a two-minute delay.
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

        // Only the install shape gates the spawn — those two can't change while the process
        // lives. The auto-check switch is read per tick instead, the welcome card and
        // Settings ▸ Updates both offering it after this point.
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
    // Independent of the daily check: the in-attempt prune only fires on the
    // next install click, so a cancelled install leaves a verified package in
    // the staging dir forever for a user who never clicks again. The grace
    // matches `updater_daily::STARTUP_DELAY`, giving the launch scan and
    // DB pre-fetch first claim on the disk.
    state.runtime.spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        services::updater::prune_stale_staging().await;
    });

    // Tarball installs only — RPM/DEB own those files through their manifests
    // and AppImage bundles them. The BLAKE3 compare means the common case is a
    // stat plus a 3 KB hash and no write. Gating rationale in the module.
    #[cfg(target_os = "linux")]
    state.runtime.spawn_blocking(|| {
        if let Err(e) = platform::desktop_integration::refresh_user_install() {
            log::warn!("desktop_integration: refresh failed: {}", describe(&e));
        }
    });

    // Restart-gated through the `restart-tray` Dialog, so this startup read is
    // the single gate; off, there is no D-Bus connection, ksni thread or
    // receiver task at all. `install` is cross-platform — Linux creates the
    // StatusNotifierItem eagerly, Win/mac defer onto the event loop.
    if settings.is_some_and(|s| s.tray.tray_enabled) {
        ui::shell::tray_bridge::install(spawner, state, app);
    }

    // Answer the launches queued on the socket claimed back at boot — here
    // rather than beside the claim, a forwarded track needing a window to raise.
    if let Some(listener) = file_open_listener {
        serve_file_opens(state, app, listener);
    }

    #[cfg(target_os = "windows")]
    {
        attach_media_controls_after_show(app, state);
        polish_titlebar_after_show(app);
    }
}

/// SMTC needs a real `HWND`, which exists only once the window is shown, so
/// `AppState::init` left the handle inert (souvlaki panics on a null one).
/// Posting to the event loop runs this on the first iteration, past the show.
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
                    // `sync()` no-op'd while the controls were inert, so the
                    // OS panel is still empty.
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

/// Boot's `theme_apply::apply` ran inside `appearance::install`, where the DWM
/// hook no-op'd with no HWND yet. By the time this one-shot fires the window
/// is up and `Theme.mantle` carries the resolved palette; every later change
/// drives DWM from `write_palette`.
///
/// The same closure pushes the embedded icon onto the winit window — the
/// taskbar reads it out of the EXE directly, but the *caption* icon comes
/// from the WNDCLASS, which winit registers with `hIcon: 0`.
#[cfg(target_os = "windows")]
fn polish_titlebar_after_show(app: &AppWindow) {
    let weak = app.as_weak();
    if let Err(e) = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        install_window_icon(&app);
        ui::appearance::theme_apply::reapply_from_theme(&app);
    }) {
        log::warn!("Failed to schedule Windows titlebar polish: {}", describe(&e));
    }
}

/// Push the embedded EXE icon onto the winit window. Windows auto-binds it to
/// the taskbar but not the caption, whose WNDCLASS winit registers with
/// `hIcon: 0`. A failure just leaves the generic icon.
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
