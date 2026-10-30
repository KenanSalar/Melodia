//! Backend failures surfaced as UI toasts from any thread. It holds no `ui` type, so `tasks` and
//! `player` raise one without naming the UI; the UI-thread consumer drains and localizes. A routine
//! failure (a favorite, a rating, a nav) is only logged, through `spawn_logged!`.

use tokio::sync::mpsc::UnboundedReceiver;

use crate::utils::event_bridge::EventBridge;

/// Which localized title the consumer gives a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// A track or station failed; the music stopping is otherwise invisible.
    PlaybackFailed,
    /// A user-initiated operation failed: a scan, an import, a settings save.
    OperationFailed,
    /// A vote didn't reach the directory, usually a repeat inside its dedup window. Said because a
    /// pressed button that quietly does nothing is worse than one that says why.
    RadioVote,
    /// A resolved sheet was written into the track's tag, a file rewrite the user asked for.
    LyricsSaved,
    /// The only sign a copy landed. No detail, so the copied text is kept nowhere else.
    Copied,
    /// A Copy entry found the field empty on every item, the menu having closed on a click that
    /// did nothing visible.
    NothingToCopy,
    /// The loved-tracks backfill queued existing favorites.
    LoveSync,
    /// A requested restart couldn't relaunch, so the app stayed up. Sticks, with no detail.
    RestartRequired,
    /// An exclusive claim fell back to shared, once per distinct refusal. The detail is the
    /// device's name; the reason stays in the Output settings, translated there.
    ExclusiveRefused,
}

/// `detail` is the untranslated body: a path or an error.
#[derive(Debug, Clone)]
pub struct ToastRequest {
    pub kind: ToastKind,
    pub detail: String,
}

static BRIDGE: EventBridge<ToastRequest> = EventBridge::new();

/// `None` if already installed, the first receiver owning delivery.
pub fn init() -> Option<UnboundedReceiver<ToastRequest>> {
    BRIDGE.install()
}

/// A no-op without a consumer. Returns nothing: a failure worth toasting has no second way to
/// reach the user.
pub fn notify(kind: ToastKind, detail: impl Into<String>) {
    BRIDGE.send(ToastRequest { kind, detail: detail.into() });
}

#[cfg(test)]
#[path = "tests/toast_tests.rs"]
mod tests;
