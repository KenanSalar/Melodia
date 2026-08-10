//! Process-wide bridge for surfacing backend failures as UI toasts.
//!
//! Backend code (`player`, `library`, `tasks`) runs on tokio workers and can't
//! touch the Slint notification stack directly (`NotificationsUi` is `Rc`,
//! UI-thread-only). This module owns a `OnceLock` [`UnboundedSender`] that any
//! thread can push a [`ToastRequest`] onto; the UI-thread consumer installed by
//! `boot::ui_setup::install_toast_bridge` drains it and renders the toast,
//! resolving the localized title from `Settings` at push time.
//!
//! Mirrors the shape of [`crate::tasks::play_count_flusher`]'s global sender: a
//! `try_send`-style API that no-ops when the bridge isn't installed (headless /
//! test contexts), so producers never have to thread a handle through their
//! call chains. Deliberately holds no `ui::*` types so the layering rule
//! (`tasks` never imports `ui`) is preserved — the producer side is UI-free and
//! the localization happens entirely on the consumer.

use std::sync::OnceLock;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Which localized template the UI consumer uses for a toast's title. A plain
/// enum (no `ui` dependency) so `player` / `library` / `tasks` can classify a
/// failure without importing UI types; the consumer maps each kind to a
/// translated `Settings` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// Playback couldn't start — a track failed to decode / open. The music
    /// stopping is otherwise invisible, so this is always surfaced.
    PlaybackFailed,
    /// A user-initiated backend operation failed (folder scan, file import,
    /// settings save, …).
    OperationFailed,
    /// The `MusicBrainz` auto-tag backfill finished a user-initiated sweep — an
    /// informational result (how many tracks were tagged), not a failure. Shown
    /// as an auto-dismissing info toast.
    MbidTagging,
    /// The retroactive loved-tracks backfill queued existing favorites after a
    /// user enabled a love toggle or connected a service — an informational
    /// result (how many were synced), shown as an auto-dismissing info toast.
    LoveSync,
}

/// A queued toast. The localized title is chosen by [`kind`](Self::kind) on the
/// UI side; [`detail`](Self::detail) is the dynamic, non-localized specifics
/// (a file path or an error message) shown as the toast body.
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
/// consumer to drain. Returns `None` if the bridge was already initialized (the
/// first receiver owns delivery) — the caller should treat that as a no-op.
pub fn init() -> Option<UnboundedReceiver<ToastRequest>> {
    let (tx, rx) = mpsc::unbounded_channel();
    match SENDER.set(tx) {
        Ok(()) => Some(rx),
        Err(_) => None,
    }
}

/// Queue a toast from any thread. No-op when the bridge isn't installed
/// (headless / test contexts) or after the consumer has shut down — sending to
/// a closed channel is silently dropped, which is the right behavior during
/// shutdown.
pub fn notify(kind: ToastKind, detail: impl Into<String>) {
    if let Some(tx) = SENDER.get() {
        let _ = tx.send(ToastRequest { kind, detail: detail.into() });
    }
}

#[cfg(test)]
#[path = "tests/toast_tests.rs"]
mod tests;
