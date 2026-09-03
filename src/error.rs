/// Boxed underlying cause for the I/O-boundary variants. `Option` because some
/// of these errors are constructed from a plain message with no wrapped error
/// (e.g. an HTTP status check that "fails" without an underlying transport
/// error). thiserror's `#[source]` returns `None` for a `None` here, so the
/// error chain is preserved when a cause exists and absent otherwise.
type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Metadata error: {msg}")]
    Metadata {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Scanner error: {msg}")]
    Scanner {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Player error: {0}")]
    Player(String),

    #[error("Queue error: {0}")]
    Queue(String),

    #[error("Settings error: {0}")]
    Settings(String),

    #[error("Window error: {0}")]
    Window(String),

    #[error("Watcher error: {msg}")]
    Watcher {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Network error: {msg}")]
    Network {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Validation error: {0}")]
    Validation(String),
}

impl AppError {
    pub fn not_found(entity: &str, id: i64) -> Self {
        Self::NotFound(format!("{entity} not found: {id}"))
    }

    pub fn io_other(msg: impl Into<String>) -> Self {
        Self::Io(std::io::Error::other(msg.into()))
    }

    /// Wrap an arbitrary error as `Io`, preserving it as the `io::Error`'s
    /// source. Unlike [`io_other`](Self::io_other) (which takes a bare message
    /// and drops the typed cause), this keeps the original error reachable via
    /// `.source()` — use it for non-io failures (`JoinError`, `serde_json`) that
    /// were previously flattened with `e.to_string()`.
    pub fn io_source(source: impl Into<BoxedSource>) -> Self {
        Self::Io(std::io::Error::other(source))
    }

    /// Metadata error wrapping an underlying cause (Lofty, hashing I/O, …).
    /// `msg` is the operation context (`"Failed to open <path>"`); the typed cause
    /// rides on `.source()` so logs can walk the chain.
    pub fn metadata(msg: impl Into<String>, source: impl Into<BoxedSource>) -> Self {
        Self::Metadata {
            msg: msg.into(),
            source: Some(source.into()),
        }
    }

    /// Metadata error from a message only (no underlying cause to preserve).
    pub fn metadata_msg(msg: impl Into<String>) -> Self {
        Self::Metadata {
            msg: msg.into(),
            source: None,
        }
    }

    /// Scanner error wrapping an underlying cause (usually a tokio `JoinError`).
    pub fn scanner(msg: impl Into<String>, source: impl Into<BoxedSource>) -> Self {
        Self::Scanner {
            msg: msg.into(),
            source: Some(source.into()),
        }
    }

    /// Scanner error from a message only.
    pub fn scanner_msg(msg: impl Into<String>) -> Self {
        Self::Scanner {
            msg: msg.into(),
            source: None,
        }
    }

    /// Watcher error wrapping an underlying cause (notify).
    pub fn watcher(msg: impl Into<String>, source: impl Into<BoxedSource>) -> Self {
        Self::Watcher {
            msg: msg.into(),
            source: Some(source.into()),
        }
    }

    /// Watcher error from a message only.
    pub fn watcher_msg(msg: impl Into<String>) -> Self {
        Self::Watcher {
            msg: msg.into(),
            source: None,
        }
    }

    /// Network error wrapping an underlying cause (reqwest / URL parse).
    pub fn network(msg: impl Into<String>, source: impl Into<BoxedSource>) -> Self {
        Self::Network {
            msg: msg.into(),
            source: Some(source.into()),
        }
    }

    /// Network error from a message only (e.g. an HTTP status or scheme check
    /// that has no underlying transport error).
    pub fn network_msg(msg: impl Into<String>) -> Self {
        Self::Network {
            msg: msg.into(),
            source: None,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Flatten an error and its causes onto one line.
///
/// A great many `Display` impls in and under this tree are a context sentence with the cause
/// reachable only through `.source()`, so a bare `{e}` reports a root-owned file and a full disk
/// in the same words.
///
/// **The other kind is what the `ends_with` skip is for, and why this is safe to reach for without
/// knowing which variant you hold.** [`AppError`]'s three `#[from]` variants spell
/// `#[error("… : {0}")]` over the field `#[from]` also makes the source, and sqlx does the same
/// one level down, so an unconditional walk prints a constraint failure three times. A caller
/// can't tell the two shapes apart; the error can — a message already ending in its cause has
/// nothing left to add.
///
/// Reach for this in any `log::` call taking an error.
pub fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        let message = source.to_string();
        if !text.ends_with(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        cause = source.source();
    }
    text
}

#[cfg(test)]
#[path = "tests/error_tests.rs"]
mod tests;
