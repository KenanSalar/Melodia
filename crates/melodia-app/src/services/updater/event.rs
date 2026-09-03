//! Backend → UI cross-thread event type.
//!
//! Constructed on tokio workers by `tasks::updater_daily` and the `Updater.install` /
//! `Updater.check` callbacks, drained by a UI-thread subscriber in
//! `ui::settings::updater_settings` that turns each variant into a
//! `NotificationsUi::show` plus the matching `Updater.*` global writes.
//!
//! A typed channel rather than poking the UI from the task, `NotificationsUi` being
//! `Rc`-backed where the task is `Send + 'static`. The variants deliberately carry no
//! translated text — the UI thread looks the strings up through
//! `Settings.invoke_update_*()`, so they re-resolve in whatever locale is active.

#[derive(Debug, Clone)]
pub enum UpdaterEvent {
    /// A strictly-newer version that hasn't been skipped. Becomes an "Update
    /// available" toast with `kind = "install-update"`, so the dispatcher in
    /// `globals/updater.slint` fires `Updater.install()` on tap.
    ///
    /// `critical` mirrors the manifest flag and hides "Skip this version". Dismissing
    /// the toast still works — it re-appears next session; only the permanent skip is
    /// suppressed.
    Available {
        version: String,
        notes_short: String,
        critical: bool,
    },
    /// `download_and_install` finished and atomically swapped the live binary.
    /// Becomes the "Update installed — Restart" toast, `kind = "update-restart"`.
    Installed,
    /// Any error path. Becomes an "Update failed" toast with a per-category message;
    /// the raw error string stays log-only, so callers classify through
    /// [`FailureKind::classify`] before sending.
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
    /// Map an [`AppError`](crate::error::AppError) onto the bucket the UI displays.
    /// A signature failure is recognised by the "signature" substring
    /// [`crate::services::updater::install`] puts on its `Validation` error; every
    /// other `Validation` reads as a parse failure.
    pub fn classify(err: &crate::error::AppError) -> Self {
        use crate::error::AppError;
        match err {
            AppError::Network { .. } => Self::Network,
            AppError::Io(_) => Self::Io,
            AppError::Validation(msg) if msg.contains("signature") => Self::Signature,
            AppError::Validation(_) => Self::Parse,
            _ => Self::Other,
        }
    }

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
