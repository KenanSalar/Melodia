//! `ListenBrainz` provider: token validation, "playing now", and durable listen
//! submission.
//!
//! Unlike Last.fm there's no app registration — the user's own token
//! authenticates every call via an `Authorization: Token <token>` header, and
//! failures come back as real HTTP statuses. Submissions carry
//! `submission_client = "Melodia"` + this build's version, and rate limiting is
//! read from the `X-RateLimit-Reset-In` header (seconds until the window
//! resets — clock-skew-proof, unlike the epoch `X-RateLimit-Reset`). The
//! functions stay unwired until Phase 2.

use serde::{Deserialize, Serialize};

use reqwest::StatusCode;

use crate::error::AppError;
use crate::services::scrobble::model::ScrobbleTrack;

/// The `ListenBrainz` API root.
const LB_API_BASE: &str = "https://api.listenbrainz.org";

/// Identifies Melodia as the player and submitting client in listen metadata.
const MEDIA_PLAYER: &str = "Melodia";
const SUBMISSION_CLIENT: &str = "Melodia";
const SUBMISSION_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A `ListenBrainz` submission's outcome, classified for the submitter's retry
/// policy.
#[derive(Debug, thiserror::Error)]
pub enum ListenBrainzError {
    /// HTTP 401 — the user token was rejected. Disconnect and prompt re-auth.
    #[error("ListenBrainz token rejected")]
    InvalidToken,
    /// HTTP 429 — rate limited. Retry after `reset_in_secs` if the header gave one.
    #[error("ListenBrainz rate limited")]
    RateLimited { reset_in_secs: Option<u64> },
    /// Any other non-success status — retry with backoff.
    #[error("ListenBrainz server error (HTTP {status}): {message}")]
    Server { status: u16, message: String },
    /// Transport or response-decode failure.
    #[error(transparent)]
    Transport(#[from] AppError),
}

/// The parsed body of `GET /1/validate-token`.
#[derive(Debug, Deserialize)]
pub struct ValidatedToken {
    pub valid: bool,
    pub user_name: Option<String>,
}

/// Validate a user token via `GET /1/validate-token`. A rejected token (HTTP
/// 401) is a normal result, not an error: it returns `valid: false`.
pub async fn validate_token(
    client: &reqwest::Client,
    token: &str,
) -> Result<ValidatedToken, ListenBrainzError> {
    let response = client
        .get(format!("{LB_API_BASE}/1/validate-token"))
        .header(reqwest::header::AUTHORIZATION, format!("Token {token}"))
        .send()
        .await
        .map_err(|e| AppError::network("ListenBrainz validate-token request failed", e))?;

    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        return Ok(ValidatedToken {
            valid: false,
            user_name: None,
        });
    }
    if !status.is_success() {
        return Err(server_error(status, response).await);
    }
    let validated = response
        .json::<ValidatedToken>()
        .await
        .map_err(|e| AppError::network("Failed to parse ListenBrainz validate-token response", e))?;
    Ok(validated)
}

/// Report the currently-playing track (no `listened_at`). Ephemeral — never
/// retried or queued.
pub async fn submit_playing_now(
    client: &reqwest::Client,
    token: &str,
    track: &ScrobbleTrack,
) -> Result<(), ListenBrainzError> {
    submit(client, token, &playing_now_payload(track)).await
}

/// Submit one or more durable listens, each stamped with its real start time.
/// An empty batch is a no-op.
pub async fn submit_listens(
    client: &reqwest::Client,
    token: &str,
    listens: &[(&ScrobbleTrack, i64)],
) -> Result<(), ListenBrainzError> {
    if listens.is_empty() {
        return Ok(());
    }
    submit(client, token, &listens_payload(listens)).await
}

/// POST a payload to `/1/submit-listens` and classify the response.
async fn submit(
    client: &reqwest::Client,
    token: &str,
    payload: &SubmitListens<'_>,
) -> Result<(), ListenBrainzError> {
    let response = client
        .post(format!("{LB_API_BASE}/1/submit-listens"))
        .header(reqwest::header::AUTHORIZATION, format!("Token {token}"))
        .json(payload)
        .send()
        .await
        .map_err(|e| AppError::network("ListenBrainz submit-listens request failed", e))?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ListenBrainzError::RateLimited {
            reset_in_secs: rate_limit_reset_in(response.headers()),
        });
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err(ListenBrainzError::InvalidToken);
    }
    Err(server_error(status, response).await)
}

/// Read a failed response's body into a `Server` error — best-effort, so an
/// unreadable body degrades to an empty message.
async fn server_error(status: StatusCode, response: reqwest::Response) -> ListenBrainzError {
    let message = response.text().await.unwrap_or_default();
    ListenBrainzError::Server {
        status: status.as_u16(),
        message,
    }
}

/// Parse `X-RateLimit-Reset-In` (seconds until the rate-limit window resets).
fn rate_limit_reset_in(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get("X-RateLimit-Reset-In")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

/// A `playing_now` payload: one listen with no `listened_at`.
fn playing_now_payload(track: &ScrobbleTrack) -> SubmitListens<'_> {
    SubmitListens {
        listen_type: "playing_now",
        payload: vec![Listen {
            listened_at: None,
            track_metadata: build_metadata(track),
        }],
    }
}

/// A durable-listen payload: `single` for one listen, `import` for a batch.
fn listens_payload<'a>(listens: &'a [(&'a ScrobbleTrack, i64)]) -> SubmitListens<'a> {
    SubmitListens {
        listen_type: if listens.len() == 1 { "single" } else { "import" },
        payload: listens
            .iter()
            .map(|(track, timestamp)| Listen {
                listened_at: Some(*timestamp),
                track_metadata: build_metadata(track),
            })
            .collect(),
    }
}

/// Build the `track_metadata` block shared by "playing now" and listen payloads.
fn build_metadata(track: &ScrobbleTrack) -> TrackMetadata<'_> {
    TrackMetadata {
        artist_name: &track.artist,
        track_name: &track.track,
        release_name: track.album.as_deref(),
        additional_info: AdditionalInfo {
            recording_mbid: track.recording_mbid.as_deref(),
            release_mbid: track.release_mbid.as_deref(),
            tracknumber: track.track_number,
            duration_ms: track.duration_secs.map(|secs| u64::from(secs) * 1000),
            media_player: MEDIA_PLAYER,
            submission_client: SUBMISSION_CLIENT,
            submission_client_version: SUBMISSION_CLIENT_VERSION,
        },
    }
}

#[derive(Serialize)]
struct SubmitListens<'a> {
    listen_type: &'static str,
    payload: Vec<Listen<'a>>,
}

#[derive(Serialize)]
struct Listen<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    listened_at: Option<i64>,
    track_metadata: TrackMetadata<'a>,
}

#[derive(Serialize)]
struct TrackMetadata<'a> {
    artist_name: &'a str,
    track_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_name: Option<&'a str>,
    additional_info: AdditionalInfo<'a>,
}

#[derive(Serialize)]
struct AdditionalInfo<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    recording_mbid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_mbid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracknumber: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    media_player: &'static str,
    submission_client: &'static str,
    submission_client_version: &'static str,
}

#[cfg(test)]
#[path = "tests/listenbrainz_tests.rs"]
mod tests;
