//! The `Updater` global's boot seed, and the subscriber turning [`UpdaterEvent`]s into toasts.
//! Every one of them is sticky, so each keeps its `Settings.invoke_update_*` text as a recipe a
//! locale switch re-runs rather than a string resolved once.

use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString, Weak};
use tokio::sync::watch;

use crate::ui::shell::notifications::{NotificationsUi, RowText};
use melodia_app::services::settings;
use melodia_app::services::updater::{UpdaterEvent, is_available};
use melodia_app::state::AppState;
use melodia_platform::services::platform::install_kind::is_system_install;
use melodia_ui::{AppWindow, MelodiaUpdater, NotificationKind, NotificationVariant, Settings};

/// Seeds the `Updater` global at boot. `updates-supported` is false on a source build, which
/// hides the whole section rather than its buttons.
pub fn install(ui: &AppWindow, state: &AppState) {
    let updater = ui.global::<MelodiaUpdater>();
    updater.set_current_version(env!("CARGO_PKG_VERSION").into());
    updater.set_updates_supported(is_available());
    updater.set_system_managed(is_system_install());
    updater.set_platform_kind(platform_kind().into());

    if let Ok(s) = settings::read_settings(&state.paths) {
        updater.set_auto_check_enabled(s.updates.auto_check_enabled);
    }
}

/// The system-managed install's package-manager hint, as the key `update-section.slint`'s ternary
/// picks its `@tr` literal by. Keep the two in step.
fn platform_kind() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Turns update events into toasts on the UI thread for as long as the window lives. `event_rx`
/// is what `tasks::updater_daily` and the `Updater.install` / `.check` callbacks publish into.
pub fn install_event_subscriber(
    weak: Weak<AppWindow>,
    notifications: Rc<NotificationsUi>,
    mut event_rx: watch::Receiver<Option<UpdaterEvent>>,
) {
    let _ = slint::spawn_local(Compat::new(async move {
        // `changed()` first: the initial `None` carries no event.
        loop {
            if event_rx.changed().await.is_err() {
                break;
            }
            let event = event_rx.borrow_and_update().clone();
            let Some(event) = event else { continue };
            let Some(ui) = weak.upgrade() else { break };
            dispatch(&ui, &notifications, event);
        }
    }));
}

fn dispatch(ui: &AppWindow, notifications: &NotificationsUi, event: UpdaterEvent) {
    match event {
        UpdaterEvent::Available { version, .. } => {
            // One card, for the newest version, however many daily checks report one.
            notifications.dismiss_by_kind(NotificationKind::InstallUpdate);
            notifications.show_localized(
                ui,
                NotificationVariant::Info,
                NotificationKind::InstallUpdate,
                move |ui| {
                    let g = ui.global::<Settings>();
                    RowText {
                        title: g.invoke_update_available_title(),
                        message: g.invoke_update_available_message(version.clone().into()),
                        action_label: g.invoke_update_install_action_label(),
                    }
                },
            );
        }
        UpdaterEvent::Installed => {
            notifications.dismiss_by_kind(NotificationKind::InstallUpdate);
            notifications.show_localized(
                ui,
                NotificationVariant::Success,
                NotificationKind::UpdateRestart,
                |ui| {
                    let g = ui.global::<Settings>();
                    RowText {
                        title: g.invoke_update_installed_title(),
                        message: g.invoke_update_installed_message(),
                        action_label: g.invoke_update_restart_action_label(),
                    }
                },
            );
        }
        UpdaterEvent::Failed { kind } => {
            // Replace any prior failure toast, or pressing Check again
            // against a still-broken network stacks identical error cards.
            notifications.dismiss_by_kind(NotificationKind::UpdateFailed);
            notifications.show_localized(
                ui,
                NotificationVariant::Error,
                NotificationKind::UpdateFailed,
                move |ui| {
                    let g = ui.global::<Settings>();
                    RowText::plain(
                        g.invoke_update_failed_title(),
                        g.invoke_update_failed_reason(SharedString::from(kind.as_kind_str())),
                    )
                },
            );
        }
    }
}
