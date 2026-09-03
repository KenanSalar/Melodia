//! Backend-to-UI bridges: the three `Signal` subscribers and the process-wide toast channel.
//!
//! Each is a closure over `ui::signal::on_signal`, which owns the subscribe-and-spawn loop; what
//! is left here is what each one does with the tick.

use std::sync::Arc;

use melodia::{AppWindow, state::AppState, ui};
use slint::ComponentHandle;

/// Keep the Tracks view in sync with scans and watcher batches. The initial `0`
/// isn't observed — `changed()` only resolves on a real `send_modify` — so this
/// doesn't race the explicit initial fetch.
///
/// Gated on section visibility, because play-count flushes bump this channel
/// after every track completion: ungated, plain listening re-fetches the whole
/// library once per song with the view hidden. While hidden the bump folds into
/// the `TracksUi` dirty flag and the section gate runs one refresh on re-enter.
pub fn install_library_changed_refresher(
    state: &AppState,
    tracks_ui: &Arc<ui::tracks::TracksUi>,
    weak: slint::Weak<AppWindow>,
) -> Result<(), melodia::error::AppError> {
    let tu = tracks_ui.clone();
    ui::signal::on_signal(&state.library_changed, weak, "library-changed", move |ui| {
        if tu.section_active() {
            ui.global::<melodia::Tracks>().invoke_request_refresh();
        } else {
            tu.mark_dirty();
        }
    })
}

/// Toast on every kernel-overflow rescan. On the UI thread so it can hold the
/// non-`Send` `Rc<NotificationsUi>` and resolve its strings at push time, in
/// whichever locale was active when the rescan fired. Coalesced upstream by the
/// `watch` slot and by `RECONCILE_IN_FLIGHT`, so a burst of overflows still
/// paints at most one toast per reconcile cycle.
pub fn install_rescan_notice_subscriber(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia::error::AppError> {
    use melodia::Settings;
    use ui::shell::notifications::RowText;

    ui::signal::on_signal(&state.rescan_notice, weak, "rescan-notice", move |ui| {
        notifications.show_localized(ui, "info", "library-resyncing", |ui| {
            let g = ui.global::<Settings>();
            RowText::plain(g.invoke_library_resyncing_title(), g.invoke_library_resyncing_message())
        });
    })
}

/// Sticky warning toast when the output device goes away — sticky for the crash
/// notice's reason: nothing else reports this and playback carries on
/// regardless, so a notice the user looked away for did nothing. No action
/// button, there being no device picker to send them to, so the kind only groups
/// the row for `dismiss_by_kind`.
pub fn install_audio_device_lost_subscriber(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia::error::AppError> {
    use melodia::Settings;
    use ui::shell::notifications::RowText;

    ui::signal::on_signal(&state.audio_device_lost, weak, "audio-device-lost", move |ui| {
        notifications.show_localized(ui, "warning", "audio-device-lost", |ui| {
            let g = ui.global::<Settings>();
            RowText::plain(g.invoke_audio_device_lost_title(), g.invoke_audio_device_lost_message())
        });
    })
}

/// Drain the process-wide `utils::toast` channel on the UI thread.
///
/// [`install_rescan_notice_subscriber`]'s shape over an `mpsc` rather than a
/// `watch` — errors must not coalesce — resolving the localized title by kind at
/// push time so a failure raised on a tokio worker paints in the locale that is
/// active when it surfaces. The dynamic detail is shown verbatim.
pub fn install_toast_bridge(
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia::error::AppError> {
    use melodia::Settings;
    use melodia::utils::toast::{self, ToastKind, ToastRequest};
    use ui::shell::notifications::{NotificationParams, RowText};

    // First installer owns delivery; a second call (shouldn't happen) is a no-op.
    let Some(mut rx) = toast::init() else {
        return Ok(());
    };
    slint::spawn_local(async_compat::Compat::new(async move {
        while let Some(ToastRequest { kind, detail }) = rx.recv().await {
            let Some(ui) = weak.upgrade() else { break };
            let g = ui.global::<Settings>();
            match kind {
                ToastKind::PlaybackFailed | ToastKind::OperationFailed => {
                    // Only the title is ours to re-render; the detail is a Rust error
                    // string that was never translated in the first place.
                    notifications.show_localized(&ui, "error", "error", move |ui| {
                        let g = ui.global::<Settings>();
                        RowText::plain(
                            match kind {
                                ToastKind::PlaybackFailed => g.invoke_toast_playback_error_title(),
                                _ => g.invoke_toast_operation_failed_title(),
                            },
                            detail.clone().into(),
                        )
                    });
                }
                // The result of a user-triggered sweep, so it auto-dismisses
                // rather than sticking like a failure.
                ToastKind::MbidTagging => {
                    notifications.show_auto_dismiss(
                        NotificationParams {
                            variant: "info".into(),
                            title: g.invoke_toast_mbid_title(),
                            message: detail.into(),
                            action_label: slint::SharedString::default(),
                            action_kind: "info".into(),
                        },
                        6000,
                    );
                }
                // A restart that had nowhere to relaunch from: no dynamic
                // detail, and it sticks because it asks the user to do
                // something rather than reporting what happened.
                ToastKind::RestartRequired => {
                    notifications.show_localized(&ui, "warning", "warning", |ui| {
                        let g = ui.global::<Settings>();
                        RowText::plain(
                            g.invoke_toast_restart_required_title(),
                            g.invoke_toast_restart_required_message(),
                        )
                    });
                }
                ToastKind::LoveSync => {
                    notifications.show_auto_dismiss(
                        NotificationParams {
                            variant: "info".into(),
                            title: g.invoke_toast_love_sync_title(),
                            message: detail.into(),
                            action_label: slint::SharedString::default(),
                            action_kind: "info".into(),
                        },
                        6000,
                    );
                }
                // A vote the directory would not take. Auto-dismissing: nothing is broken
                // and there is nothing for the user to do about it.
                ToastKind::RadioVote => {
                    notifications.show_auto_dismiss(
                        NotificationParams {
                            variant: "warning".into(),
                            title: g.invoke_toast_radio_vote_title(),
                            message: detail.into(),
                            action_label: slint::SharedString::default(),
                            action_kind: "warning".into(),
                        },
                        6000,
                    );
                }
            }
        }
    }))
    .map(|_| ())
    .map_err(|e| melodia::error::AppError::Window(format!("toast bridge: {e}")))
}
