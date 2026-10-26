/// Boxed underlying cause for the variants that carry one. `Option` because some
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

    #[error("Not found: {msg}")]
    NotFound { msg: String },

    #[error("Player error: {msg}")]
    Player {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Queue error: {msg}")]
    Queue {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Settings error: {msg}")]
    Settings {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Window error: {msg}")]
    Window {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

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

    #[error("Updater error: {msg}")]
    Updater {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },

    #[error("Validation error: {msg}")]
    Validation {
        msg: String,
        #[source]
        source: Option<BoxedSource>,
    },
}

/// What [`AppError::io`] hands `io::Error`: the operation and the error that failed it.
#[derive(Debug, thiserror::Error)]
#[error("{msg}: {source}")]
struct Context {
    msg: String,
    source: BoxedSource,
}

impl AppError {
    pub fn not_found(entity: &str, id: i64) -> Self {
        Self::not_found_msg(format!("{entity} not found: {id}"))
    }

    /// Not found from a message only. A lookup miss has no cause, so there is no source form.
    pub fn not_found_msg(msg: impl Into<String>) -> Self {
        Self::NotFound { msg: msg.into() }
    }

    pub fn io_other(msg: impl Into<String>) -> Self {
        Self::Io(std::io::Error::other(msg.into()))
    }

    /// Wrap an arbitrary error as `Io`, kept typed as the `io::Error`'s payload. Reach for
    /// [`io`](Self::io) instead wherever there is an operation to name.
    pub fn io_source(source: impl Into<BoxedSource>) -> Self {
        Self::Io(std::io::Error::other(source))
    }

    /// `Io` naming the operation that failed beside its typed cause. The home for a cause no
    /// category fits, such as a task join.
    ///
    /// Both halves are printed, so a bare `{e}` still says why, and [`describe`] skips the repeat
    /// on its way down the chain.
    pub fn io(msg: impl Into<String>, source: impl Into<BoxedSource>) -> Self {
        Self::Io(std::io::Error::other(Context { msg: msg.into(), source: source.into() }))
    }
}

/// The two constructors every cause-carrying variant gets: `$cause(msg, source)`, with `msg`
/// naming the operation and the typed cause kept on `.source()` for [`describe`] to walk, and
/// `$message(msg)` for a failure with no error underneath it.
macro_rules! constructor_pairs {
    ($($variant:ident => $cause:ident, $message:ident;)+) => {
        impl AppError {
            $(
                pub fn $cause(msg: impl Into<String>, source: impl Into<BoxedSource>) -> Self {
                    Self::$variant { msg: msg.into(), source: Some(source.into()) }
                }

                pub fn $message(msg: impl Into<String>) -> Self {
                    Self::$variant { msg: msg.into(), source: None }
                }
            )+
        }
    };
}

constructor_pairs! {
    Metadata => metadata, metadata_msg;
    Scanner => scanner, scanner_msg;
    Player => player, player_msg;
    Queue => queue, queue_msg;
    Settings => settings, settings_msg;
    Window => window, window_msg;
    Watcher => watcher, watcher_msg;
    Network => network, network_msg;
    Updater => updater, updater_msg;
    Validation => validation, validation_msg;
}

pub type AppResult<T> = Result<T, AppError>;

/// Flatten an error and its causes onto one line.
///
/// A great many `Display` impls in and under this tree are a context sentence with the cause
/// reachable only through `.source()`, so a bare `{e}` reports a root-owned file and a full disk
/// in the same words.
///
/// **The other kind is what the skip is for, and why this is safe to reach for without knowing
/// which variant you hold.** [`AppError`]'s three `#[from]` variants spell `#[error("…: {0}")]`
/// over the field `#[from]` also makes the source, and sqlx does the same one level down, so an
/// unconditional walk prints a constraint failure three times. A caller can't tell the two shapes
/// apart, but the error can: a message already ending in its cause has nothing left to add.
///
/// Reach for this in any `log::` call taking an error; `crates/melodia/tests/logged_errors.rs`
/// fails on one that doesn't.
pub fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        let message = source.to_string();
        if !already_printed(&text, &message) {
            text.push_str(CAUSE_SEPARATOR);
            text.push_str(&message);
        }
        cause = source.source();
    }
    text
}

/// What [`describe`] joins a cause on, and what every `#[error]` in the tree interpolating its
/// cause puts before it.
const CAUSE_SEPARATOR: &str = ": ";

/// Whether `text` already carries `message` as its cause: the whole of it, or behind the separator.
/// A bare suffix match would drop a cause whose words merely end the context sentence. An empty
/// cause counts as printed, there being nothing to append but the separator.
fn already_printed(text: &str, message: &str) -> bool {
    message.is_empty()
        || text == message
        || text.strip_suffix(message).is_some_and(|head| head.ends_with(CAUSE_SEPARATOR))
}

#[cfg(test)]
#[path = "tests/error_tests.rs"]
mod tests;
