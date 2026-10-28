//! The lyrics directories, and the one call the app makes onto them. A caller gets a
//! [`LyricsAnswer`] or nothing, never the service's wire shape or which service answered.
//!
//! [`recording`] is shared rather than per client: whether a row is the recording we hold tags
//! for is a question about tags, not about any one service.

mod lrclib;
mod recording;

use std::fmt;
use std::time::Duration;

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::lyrics::LyricsAnswer;
use melodia_core::error::AppError;

use super::pacer::RequestPacer;

/// The middle of the 200–500 ms lrclib documents between requests. Compliance rather than tuning:
/// its limiter is per address, so one user is nowhere near it, but a misbehaving client is banned
/// by `User-Agent`, which is every install at once.
const LRCLIB_REQUEST_INTERVAL: Duration = Duration::from_millis(350);

/// The lyrics directory's pacer, for the caller to own.
#[must_use]
pub fn pacer() -> RequestPacer {
    RequestPacer::new(LRCLIB_REQUEST_INTERVAL)
}

/// Why a lookup produced no answer. A refusal for a stated period stands apart because nothing
/// but waiting fixes it, and `AppError` can't carry the period.
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
            Self::RateLimited { retry_after: Some(wait) } => {
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

/// Asks the directory about one track. The whole credit, because the signature asks with its
/// printed line and the index with the names behind it.
pub async fn fetch(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    credit: &ArtistCredit,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, LookupError> {
    lrclib::fetch(client, pacer, title, credit, album, duration_ms).await
}
