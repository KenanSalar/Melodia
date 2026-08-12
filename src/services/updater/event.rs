//! Backend → UI cross-thread event type.
//!
//! Constructed by `tasks::updater_daily` and the `Updater.install` /
//! `Updater.check` callbacks (both run on tokio workers), drained by a
//! UI-thread subscriber installed in `ui::settings::updater_settings`. The
//! subscriber translates each variant into a `NotificationsUi::show`
//! call plus the matching `Updater.*` global writes.
//!
//! Why an enum rather than directly poking the UI from the task:
//! `NotificationsUi` is `Rc`-backed (UI-thread-only); the tokio task
//! is `Send + 'static`. Crossing the boundary needs either a thread-safe
//! wrapper around the notifications handle (intrusive) or a typed
//! channel (what this is). The variants intentionally don't carry
//! translated text — the UI thread looks the strings up via
//! `Settings.invoke_update_*()` so they re-resolve in whatever locale
//! is currently active.

#[derive(Debug, Clone)]
pub enum UpdaterEvent {
    /// `check_for_update` reported a strictly-newer version that
    /// hasn't been skipped. UI translates → "Update available" toast
    /// with `kind = "install-update"` so the dispatcher in
    /// `melodia-ui/ui/globals/updater.slint` fires `Updater.install()` on tap.
    ///
    /// `critical = true` mirrors the manifest's `critical` flag so the
    /// Settings → Updates panel hides the "Skip this version" button.
    /// Toast dismissal still works (re-appears next session); only the
    /// permanent skip is suppressed.
    Available {
        version: String,
        notes_short: String,
        critical: bool,
    },
    /// `download_and_install` finished + atomically swapped the live
    /// binary. UI translates → "Update installed — Restart" toast
    /// with `kind = "update-restart"`.
    Installed,
    /// Any error path (network, signature, install). UI translates →
    /// "Update failed" toast with a friendly per-category message —
    /// the raw Rust error string was log-only since [`FailureKind`]
    /// replaced the previous `reason: String` field. Callers classify
    /// the underlying `AppError` via [`FailureKind::classify`] before
    /// sending.
    Failed { kind: FailureKind },
}

/// Categorises an updater error into the bucket the UI uses to pick the
/// toast message. Kept narrow on purpose — the toast is a one-liner;
/// when the user wants more they look in `RUST_LOG=info` output where
/// the raw `AppError` was already logged at the send site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Couldn't reach the update server (DNS, TCP, TLS, captive portal,
    /// HTTP 5xx, body decode IO).
    Network,
    /// Signature verification failed — the downloaded artifact doesn't
    /// match the embedded public key. Treat as adversarial until proven
    /// otherwise (corrupted CDN, MITM, mis-signed release).
    Signature,
    /// Manifest didn't parse, or parsed but contained unexpected fields
    /// (e.g. asset URL missing for a platform key the manifest itself
    /// declares is supported).
    Parse,
    /// Local I/O error — couldn't write the staged file, couldn't
    /// rename, couldn't clean up. Often disk-full, permissions, or AV
    /// holding the file.
    Io,
    /// Anything not covered above. Generic "Update failed" message.
    Other,
}

impl FailureKind {
    /// Map an [`AppError`](crate::error::AppError) onto the bucket the
    /// UI displays. Signature failures are detected via the explicit
    /// "signature" substring the [`crate::services::updater::install`]
    /// site puts on its `Validation` error — every other `Validation`
    /// bucket is treated as a parse error.
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

    /// Stable string discriminator handed to Slint so the
    /// `Settings.update-failed-reason(kind)` callback can branch on it.
    /// Don't change the values without updating the Slint switch too.
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
