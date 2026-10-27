//! The updater's event for the UI thread, toasted by `ui::settings::updater_settings`, and the
//! failure every updater path returns.
//!
//! The variants carry no translated text: the UI thread looks each string up through
//! `Settings.invoke_update_*()`, so it follows the active locale.

use melodia_core::error::AppError;

#[derive(Debug, Clone)]
pub enum UpdaterEvent {
    /// A newer version not skipped, toasted as `InstallUpdate`. `critical` hides only "Skip this
    /// version"; a dismissed toast still returns at the next daily check.
    Available { version: String, notes_short: String, critical: bool },
    /// The next launch runs the new version, toasted as `UpdateRestart`.
    Installed,
    /// Toasted by its kind; the error itself stays in the log.
    Failed { kind: FailureKind },
}

/// The bucket a failure's one-line toast comes from, the error having been logged at the send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Couldn't reach the update server — DNS, TCP, TLS, captive portal, HTTP 5xx.
    Network,
    /// The artifact doesn't match the embedded public key. Treat as adversarial until
    /// proven otherwise — corrupted CDN, MITM, mis-signed release.
    Signature,
    /// The manifest didn't parse, or parsed missing something it declares it has.
    Parse,
    /// Local I/O: the staged write, the rename, the cleanup.
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
