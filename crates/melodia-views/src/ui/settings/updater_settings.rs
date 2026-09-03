//! Updater UI glue — startup seeding of the `Updater` Slint global +
//! the `Notifications`-pushing subscriber for backend
//! [`UpdaterEvent`]s.
//!
//! Composition:
//!
//! - [`install`] runs once at boot. Seeds `Updater.current-version` from
//!   `CARGO_PKG_VERSION`, `Updater.auto-check-enabled` from disk,
//!   `Updater.system-managed` from the writability probe in
//!   `services::platform::install_kind::system_install`, and `Updater.updates-supported`
//!   from `services::updater::is_available` — false on a source build,
//!   which takes the whole section rather than trading its buttons.
//! - [`install_event_subscriber`] hooks `event_rx` onto a UI-thread
//!   `slint::spawn_local` loop that translates each `UpdaterEvent`
//!   variant into a [`NotificationsUi::show_localized`] call. All three
//!   rows are sticky, so the `Settings.invoke_update_*()` callbacks are
//!   kept as a recipe rather than resolved once at push time.

use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, SharedString, Weak};
use tokio::sync::watch;

use crate::ui::shell::notifications::{NotificationsUi, RowText};
use melodia_app::services::settings;
use melodia_app::services::updater::{UpdaterEvent, is_available, is_system_install};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, MelodiaUpdater, Settings};

// (FailureKind classification is performed at the send site in
// `ui::callbacks::updater`; here we just hand the discriminator string
// to Slint so the @tr() table can pick the right friendly message.)

/// Seed the `Updater` global at startup. Idempotent — safe to call
/// before any user interaction.
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

/// Discriminator string for the package-manager hint shown to users on
/// system-managed installs. The Slint side branches on the value to
/// pick the right `@tr(...)` literal so the hint re-resolves on locale
/// switch. Keep the values in sync with the ternary in
/// `crates/melodia-ui/ui/views/settings/update-section.slint`.
fn platform_kind() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Spawn the UI-thread subscriber that turns backend update events into
/// toast notifications. Runs as a single `slint::spawn_local` task
/// that exits when the `AppWindow` weak handle can no longer be
/// upgraded (window dropped).
///
/// `event_rx` is the consumer end of the `watch` channel
/// `tasks::updater_daily::spawn` (and the `Updater.install` /
/// `Updater.check` callbacks) write into.
pub fn install_event_subscriber(
    weak: Weak<AppWindow>,
    notifications: Rc<NotificationsUi>,
    mut event_rx: watch::Receiver<Option<UpdaterEvent>>,
) {
    let _ = slint::spawn_local(Compat::new(async move {
        // Skip the initial `None` value — it carries no event.
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
            // Replace any prior update-available toast — only show the
            // most-recently-reported version. (Without this, two
            // successive daily checks could stack two cards.)
            notifications.dismiss_by_kind("install-update");
            notifications.show_localized(ui, "info", "install-update", move |ui| {
                let g = ui.global::<Settings>();
                RowText {
                    title: g.invoke_update_available_title(),
                    message: g.invoke_update_available_message(version.clone().into()),
                    action_label: g.invoke_update_install_action_label(),
                }
            });
        }
        UpdaterEvent::Installed => {
            notifications.dismiss_by_kind("install-update");
            notifications.show_localized(ui, "success", "update-restart", |ui| {
                let g = ui.global::<Settings>();
                RowText {
                    title: g.invoke_update_installed_title(),
                    message: g.invoke_update_installed_message(),
                    action_label: g.invoke_update_restart_action_label(),
                }
            });
        }
        UpdaterEvent::Failed { kind } => {
            // Replace any prior failure toast — repeated failures
            // (e.g. the daily task wakes up to a still-broken DNS)
            // would otherwise stack identical error cards.
            notifications.dismiss_by_kind("update-failed");
            notifications.show_localized(ui, "error", "update-failed", move |ui| {
                let g = ui.global::<Settings>();
                RowText::plain(
                    g.invoke_update_failed_title(),
                    g.invoke_update_failed_reason(SharedString::from(kind.as_kind_str())),
                )
            });
        }
    }
}
