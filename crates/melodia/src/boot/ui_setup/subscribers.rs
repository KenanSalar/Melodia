//! Backend-to-UI bridges, each run on the UI thread, plus the memory sampler, whose loop reads
//! the UI.

use std::sync::Arc;

use melodia_app::state::AppState;
use melodia_app::tasks;
use melodia_ui::AppWindow;
use melodia_views::ui;
use slint::ComponentHandle;

/// Reloads the Songs list on a library change while it is on screen, else marks it dirty for its
/// next enter: every favourite, rating, tag edit and watcher batch bumps this channel.
pub fn install_library_changed_refresher(
    state: &AppState,
    tracks_ui: &Arc<ui::tracks::TracksUi>,
    weak: slint::Weak<AppWindow>,
) -> Result<(), melodia_core::error::AppError> {
    let tu = tracks_ui.clone();
    ui::signal::on_signal(&state.library_changed, weak, "library-changed", move |ui| {
        if tu.section_active() {
            ui.global::<melodia_ui::Tracks>().invoke_request_refresh();
        } else {
            tu.mark_dirty();
        }
    })
}

/// Toasts a kernel-overflow rescan. A burst of overflows coalesces upstream into one toast per
/// reconcile.
pub fn install_rescan_notice_subscriber(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia_core::error::AppError> {
    use melodia_ui::{NotificationKind, NotificationVariant, Settings};
    use ui::shell::notifications::RowText;

    ui::signal::on_signal(&state.rescan_notice, weak, "rescan-notice", move |ui| {
        notifications.show_localized(
            ui,
            NotificationVariant::Info,
            NotificationKind::LibraryResyncing,
            |ui| {
                let g = ui.global::<Settings>();
                RowText::plain(
                    g.invoke_library_resyncing_title(),
                    g.invoke_library_resyncing_message(),
                )
            },
        );
    })
}

/// Raises a sticky warning when the output device goes away: nothing else reports it, and
/// playback carries on regardless.
pub fn install_audio_device_lost_subscriber(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia_core::error::AppError> {
    use melodia_ui::{NotificationKind, NotificationVariant, Settings};
    use ui::shell::notifications::RowText;

    ui::signal::on_signal(&state.audio_device_lost, weak, "audio-device-lost", move |ui| {
        notifications.show_localized(
            ui,
            NotificationVariant::Warning,
            NotificationKind::AudioDeviceLost,
            |ui| {
                let g = ui.global::<Settings>();
                RowText::plain(
                    g.invoke_audio_device_lost_title(),
                    g.invoke_audio_device_lost_message(),
                )
            },
        );
    })
}

/// Holds an info toast up while a scan restores cover art whose stored file went missing.
///
/// A payload loop, not [`ui::signal::on_signal`]: the boot scan can raise the count before this
/// attaches. Acts only on edges, so a second restore joining the first stacks no second card.
pub fn install_artwork_restore_subscriber(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia_core::error::AppError> {
    use melodia_ui::{NotificationKind, NotificationVariant, Settings};
    use ui::shell::notifications::RowText;

    let mut restoring = state.artwork_restoring.subscribe();
    slint::spawn_local(async_compat::Compat::new(async move {
        let mut shown = false;
        loop {
            let active = *restoring.borrow_and_update() > 0;
            if active != shown {
                let Some(ui) = weak.upgrade() else { break };
                if active {
                    notifications.show_localized(
                        &ui,
                        NotificationVariant::Info,
                        NotificationKind::ArtworkRestoring,
                        |ui| {
                            let g = ui.global::<Settings>();
                            RowText::plain(
                                g.invoke_artwork_restoring_title(),
                                g.invoke_artwork_restoring_message(),
                            )
                        },
                    );
                } else {
                    notifications.dismiss_by_kind(NotificationKind::ArtworkRestoring);
                }
                shown = active;
            }
            if restoring.changed().await.is_err() {
                break;
            }
        }
    }))
    .map(|_| ())
    .map_err(|e| melodia_core::error::AppError::io("artwork restore subscriber", e))
}

/// Drains the process-wide `utils::toast` channel, an `mpsc` because errors must not coalesce.
pub fn install_toast_bridge(
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia_core::error::AppError> {
    // First installer owns delivery; a second call (shouldn't happen) is a no-op.
    let Some(mut rx) = melodia_core::utils::toast::init() else {
        return Ok(());
    };
    slint::spawn_local(async_compat::Compat::new(async move {
        while let Some(request) = rx.recv().await {
            let Some(ui) = weak.upgrade() else { break };
            show_toast(&ui, &notifications, request);
        }
    }))
    .map(|_| ())
    .map_err(|e| melodia_core::error::AppError::io("toast bridge", e))
}

/// Titles resolve by kind in the live locale; the detail is shown verbatim.
fn show_toast(
    ui: &AppWindow,
    notifications: &ui::shell::notifications::NotificationsUi,
    request: melodia_core::utils::toast::ToastRequest,
) {
    use melodia_core::utils::toast::ToastKind;
    use melodia_ui::{NotificationKind, NotificationVariant, Settings};
    use ui::shell::notifications::{NotificationParams, RowText};

    let melodia_core::utils::toast::ToastRequest { kind, detail } = request;
    let g = ui.global::<Settings>();
    match kind {
        ToastKind::PlaybackFailed | ToastKind::OperationFailed => {
            notifications.show_failure(ui, move |ui| {
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
        // Sticky: it asks the user to do something rather than reporting what happened.
        ToastKind::RestartRequired => {
            notifications.show_localized(
                ui,
                NotificationVariant::Warning,
                NotificationKind::None,
                |ui| {
                    let g = ui.global::<Settings>();
                    RowText::plain(
                        g.invoke_toast_restart_required_title(),
                        g.invoke_toast_restart_required_message(),
                    )
                },
            );
        }
        ToastKind::WatchLimit => show_watch_limit(ui, notifications),
        ToastKind::FolderUnwatched => show_folder_unwatched(ui, notifications, detail),
        ToastKind::LoveSync => {
            notifications.show_auto_dismiss(
                NotificationParams::plain(
                    NotificationVariant::Info,
                    g.invoke_toast_love_sync_title(),
                    detail.into(),
                ),
                6000,
            );
        }
        // Confirms a write the user asked for, on the sheet they are looking at.
        ToastKind::LyricsSaved => {
            notifications.show_auto_dismiss(
                NotificationParams::plain(
                    NotificationVariant::Info,
                    g.invoke_toast_lyrics_saved_title(),
                    detail.into(),
                ),
                4000,
            );
        }
        // Confirms what the user just did, so shorter still.
        ToastKind::Copied | ToastKind::NothingToCopy => {
            let title = match kind {
                ToastKind::Copied => g.invoke_toast_copied_title(),
                _ => g.invoke_toast_nothing_to_copy_title(),
            };
            notifications.show_auto_dismiss(
                NotificationParams::plain(
                    NotificationVariant::Info,
                    title,
                    slint::SharedString::default(),
                ),
                2500,
            );
        }
        // The music carries on through the shared output, and Output settings say why.
        ToastKind::ExclusiveRefused => {
            notifications.show_auto_dismiss(
                NotificationParams {
                    variant: NotificationVariant::Warning,
                    title: g.invoke_toast_exclusive_refused_title(),
                    message: g.invoke_toast_exclusive_refused_message(detail.into()),
                    action_label: g.invoke_toast_exclusive_refused_action_label(),
                    kind: NotificationKind::ExclusiveRefused,
                },
                6000,
            );
        }
        // A refused vote: nothing is broken, and nothing is for the user to do.
        ToastKind::RadioVote => {
            notifications.show_auto_dismiss(
                NotificationParams::plain(
                    NotificationVariant::Warning,
                    g.invoke_toast_radio_vote_title(),
                    detail.into(),
                ),
                6000,
            );
        }
    }
}

/// Sticky, as it asks the user to act, and one card at most: a repeat replaces it rather than
/// stacking a second.
fn show_watch_limit(ui: &AppWindow, notifications: &ui::shell::notifications::NotificationsUi) {
    use melodia_ui::{NotificationKind, NotificationVariant, Settings};

    notifications.dismiss_by_kind(NotificationKind::WatchLimit);
    notifications.show_localized(
        ui,
        NotificationVariant::Warning,
        NotificationKind::WatchLimit,
        |ui| {
            let g = ui.global::<Settings>();
            ui::shell::notifications::RowText::plain(
                g.invoke_toast_watch_limit_title(),
                g.invoke_toast_watch_limit_message(),
            )
        },
    );
}

/// Sticky and one card at most, as [`show_watch_limit`], the newest folder replacing the last.
fn show_folder_unwatched(
    ui: &AppWindow,
    notifications: &ui::shell::notifications::NotificationsUi,
    folder: String,
) {
    use melodia_ui::{NotificationKind, NotificationVariant, Settings};

    notifications.dismiss_by_kind(NotificationKind::FolderUnwatched);
    notifications.show_localized(
        ui,
        NotificationVariant::Warning,
        NotificationKind::FolderUnwatched,
        move |ui| {
            ui::shell::notifications::RowText::plain(
                ui.global::<Settings>().invoke_toast_folder_unwatched_title(),
                folder.clone().into(),
            )
        },
    );
}

/// Run the memory sampler on the UI thread, where its tag reads the Nav and detail globals with
/// no atomic shadow. A no-op unless `MELODIA_RSS_SAMPLE` is set.
pub fn install_rss_sampler(
    weak: slint::Weak<AppWindow>,
) -> Result<(), melodia_core::error::AppError> {
    let view_tag = move || weak.upgrade().map(|ui| ui::view_tag::format_view(&ui));
    let Some(sampler) = tasks::rss_sampler::sampler(view_tag) else {
        return Ok(());
    };
    slint::spawn_local(async_compat::Compat::new(sampler))
        .map(|_| ())
        .map_err(|e| melodia_core::error::AppError::io("memory sampler", e))
}
