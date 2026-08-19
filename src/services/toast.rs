//! Process-wide bridge for surfacing backend failures as UI toasts.
//!
//! Backend code runs on tokio workers and can't touch the Slint notification stack
//! directly, `NotificationsUi` being `Rc`. So this owns a `OnceLock`
//! [`UnboundedSender`] any thread can push a [`ToastRequest`] onto, drained by the
//! UI-thread consumer `boot::ui_setup::install_toast_bridge` installs.
//!
//! It holds no `ui::*` types, which is what preserves the layering rule that `tasks`
//! never imports `ui` — the producer side is UI-free and localization happens entirely
//! on the consumer. Like [`crate::tasks::play_count_flusher`]'s global sender it
//! no-ops when uninstalled, so producers never thread a handle through their call
//! chains.

use std::sync::OnceLock;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Which localized template the UI consumer titles a toast with. A plain enum with no
/// `ui` dependency, so `player` / `library` / `tasks` classify a failure without
/// importing UI types and the consumer maps each kind to a translated string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// A track failed to decode or open. The music stopping is otherwise invisible, so
    /// this is always surfaced.
    PlaybackFailed,
    /// A user-initiated backend operation failed — folder scan, file import, settings
    /// save.
    OperationFailed,
    /// A user-initiated `MusicBrainz` auto-tag sweep finished. Informational (how many
    /// tracks were tagged) rather than a failure, so it auto-dismisses.
    MbidTagging,
    /// The retroactive loved-tracks backfill queued existing favorites after a love
    /// toggle or a connect. Informational, auto-dismissing.
    LoveSync,
    /// A restart the user asked for couldn't relaunch the app, so it stayed up instead
    /// of exiting into nothing. The setting is already persisted, so this is an
    /// instruction rather than a failure report: it sticks, and it is the one kind
    /// carrying **no** detail — there being no path or error worth showing.
    RestartRequired,
}

/// A queued toast. [`kind`](Self::kind) picks the localized title on the UI side;
/// [`detail`](Self::detail) is the dynamic, untranslated body — a path or an error.
#[derive(Debug, Clone)]
pub struct ToastRequest {
    pub kind: ToastKind,
    pub detail: String,
}

/// Process-wide sender, set once by [`init`]. Read by [`fn@notify`] so producers
/// don't have to carry a channel handle. Unset in tests → [`fn@notify`] no-ops.
/// (Disambiguated: the `notify` crate is a dependency of this build.)
static SENDER: OnceLock<UnboundedSender<ToastRequest>> = OnceLock::new();

/// Register the process-wide sender and hand back the receiver for the UI-thread
/// consumer to drain. `None` if the bridge was already initialized — the first
/// receiver owns delivery, so the caller treats that as a no-op.
pub fn init() -> Option<UnboundedReceiver<ToastRequest>> {
    let (tx, rx) = mpsc::unbounded_channel();
    match SENDER.set(tx) {
        Ok(()) => Some(rx),
        Err(_) => None,
    }
}

/// Queue a toast from any thread. No-op when the bridge isn't installed or after the
/// consumer has shut down — a send to a closed channel is dropped, which is the right
/// behaviour during shutdown.
pub fn notify(kind: ToastKind, detail: impl Into<String>) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(ToastRequest {
            kind,
            detail: detail.into(),
        });
    }
}

#[cfg(test)]
#[path = "tests/toast_tests.rs"]
mod tests;
