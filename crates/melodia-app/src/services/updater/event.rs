//! Backend → UI cross-thread event type, and the failure every updater path returns.
//!
//! Constructed on tokio workers by both update checks and the `Updater.install`
//! callback, drained by a UI-thread subscriber in
//! `ui::settings::updater_settings` that turns each variant into a toast.
//!
//! A typed channel rather than poking the UI from the task, `NotificationsUi` being
//! `Rc`-backed where the task is `Send + 'static`. The variants deliberately carry no
//! translated text — the UI thread looks the strings up through
//! `Settings.invoke_update_*()`, so they re-resolve in whatever locale is active.

use melodia_core::error::AppError;

#[derive(Debug, Clone)]
pub enum UpdaterEvent {
    /// A strictly-newer version that hasn't been skipped. Becomes an "Update
    /// available" toast of kind `InstallUpdate`, whose button fires
    /// `MelodiaUpdater.install()`.
    ///
    /// `critical` mirrors the manifest flag and hides "Skip this version". Dismissing
    /// the toast still works: it returns at the next daily check, and only the
    /// permanent skip is suppressed.
    Available { version: String, notes_short: String, critical: bool },
    /// `download_and_install` finished and the next launch runs the new version.
    /// Becomes the "Update installed — Restart" toast, of kind `UpdateRestart`.
    Installed,
    /// Any error path. Becomes an "Update failed" toast with a per-category message,
    /// the kind its [`UpdateFailure`] was made with; the error itself stays log-only.
    Failed { kind: FailureKind },
}

/// The bucket the UI picks a toast message from. Narrow on purpose — the toast is a
/// one-liner, and the raw `AppError` was already logged at the send site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Couldn't reach the update server — DNS, TCP, TLS, captive portal, HTTP 5xx.
    Network,
    /// The artifact doesn't match the embedded public key. Treat as adversarial until
    /// proven otherwise — corrupted CDN, MITM, mis-signed release.
    Signature,
    /// The manifest didn't parse, or parsed missing something it declares it has.
    Parse,
    /// Local I/O — the staged write, the rename, the cleanup. Usually disk-full,
    /// permissions, or AV holding the file.
    Io,
    /// Anything else; a generic "Update failed".
    Other,
}

impl FailureKind {
    /// Stable discriminator handed to Slint for `Settings.update-failed-reason(kind)`
    /// to branch on. Don't change a value without the Slint switch beside it.
    pub fn as_kind_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Signature => "signature",
            Self::Parse => "parse",
            Self::Io => "io",
            Self::Other => "other",
        }
    }
}

/// An updater failure, its toast bucket decided where the error was made rather than read back
/// off the error's text. The kind travels beside the [`AppError`]; it doesn't replace it.
#[derive(Debug)]
pub struct UpdateFailure {
    pub kind: FailureKind,
    pub error: AppError,
}

pub type UpdateResult<T> = Result<T, UpdateFailure>;

impl UpdateFailure {
    pub fn signature(error: AppError) -> Self {
        Self { kind: FailureKind::Signature, error }
    }

    pub fn parse(error: AppError) -> Self {
        Self { kind: FailureKind::Parse, error }
    }
}

/// The kind of a failure no updater site named: a fetch or a file operation failing below the
/// updater keeps its own category, and everything else, the updater's own errors included, takes
/// the generic toast.
impl From<AppError> for UpdateFailure {
    fn from(error: AppError) -> Self {
        let kind = match &error {
            AppError::Network { .. } => FailureKind::Network,
            AppError::Io(_) => FailureKind::Io,
            _ => FailureKind::Other,
        };
        Self { kind, error }
    }
}

impl std::fmt::Display for UpdateFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

/// Transparent, so `describe` walks the same chain off the failure as off its error.
impl std::error::Error for UpdateFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

#[cfg(test)]
#[path = "tests/event_tests.rs"]
mod tests;
