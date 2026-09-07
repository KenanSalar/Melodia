//! The lyrics directories, and the one call the app makes onto them.
//!
//! A caller hands over the tags it has and gets a [`LyricsAnswer`] or nothing back, learning
//! neither which service answered nor what its wire shape was. That is `radio_browser`'s split for
//! its reason: the response types stay private to this tree, and what crosses is a domain answer.
//!
//! **The matcher is shared and the clients are not.** [`recording`] answers "is this row the
//! recording we are holding tags for", which is a question about tags rather than about any one
//! service, so a second provider takes it rather than copying it.

mod lrclib;
mod recording;

use std::fmt;
use std::time::Duration;

use melodia_core::entities::lyrics::LyricsAnswer;
use melodia_core::error::AppError;

use super::pacer::RequestPacer;

/// The delay lrclib's own documentation asks clients to leave between requests, taken from the
/// middle of the 200-500 ms range it names.
///
/// **Compliance rather than tuning.** The limiter behind the service is keyed on the client
/// address, so one user is nowhere near the ceiling on their own and a headroom calculation would
/// conclude this is unnecessary. What is not per-address is the ban: a client that misbehaves is
/// blocked by `User-Agent`, which is every install of it at once. It also spaces the two requests
/// a single lookup can make, which the same paragraph asks for by name.
const LRCLIB_REQUEST_INTERVAL: Duration = Duration::from_millis(350);

/// A fresh pacer for the lyrics directory.
///
/// Handed out here so the interval stays beside the client it describes, and constructed by the
/// caller so it is owned state rather than a `static` the tests cannot reset.
#[must_use]
pub fn pacer() -> RequestPacer {
    RequestPacer::new(LRCLIB_REQUEST_INTERVAL)
}

/// Why a lookup produced no answer.
///
/// Two arms because they mean different things to the caller: one is the directory declining to
/// serve us for a stated period, which nothing but waiting fixes, and the other is everything
/// else. `AppError` cannot carry the period, and an error is never carried as a `String`, so this
/// is the local error type that rule leaves room for.
#[derive(Debug)]
pub enum LookupError {
    /// The directory asked us to stop, for this long where it said how long.
    RateLimited {
        retry_after: Option<Duration>,
    },
    Failed(AppError),
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited {
                retry_after: Some(wait),
            } => {
                write!(f, "The lyrics directory is rate limiting us for {} s", wait.as_secs())
            }
            Self::RateLimited { retry_after: None } => {
                write!(f, "The lyrics directory is rate limiting us")
            }
            Self::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LookupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Failed(e) => Some(e),
            Self::RateLimited { .. } => None,
        }
    }
}

/// Ask the directory about one track.
pub async fn fetch(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    artist: &str,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, LookupError> {
    lrclib::fetch(client, pacer, title, artist, album, duration_ms).await
}
