//! `ListenBrainz` provider: token validation, "playing now", and durable listen
//! submission.
//!
//! Unlike Last.fm there's no app registration — the user's own token
//! authenticates every call via an `Authorization: Token <token>` header, and
//! failures come back as real HTTP statuses. Submissions carry
//! `submission_client = "Melodia"` + this build's version, and rate limiting is
//! read from the `X-RateLimit-Reset-In` header (seconds until the window
//! resets — clock-skew-proof, unlike the epoch `X-RateLimit-Reset`).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use reqwest::StatusCode;

use crate::error::AppError;
use crate::services::integrations::scrobble::model::ScrobbleTrack;

/// The `ListenBrainz` API root.
const LB_API_BASE: &str = "https://api.listenbrainz.org";

/// Ceiling on a refusal's body. Tighter than [`ANSWER_MAX_BYTES`] because the text lands in an
/// error a user reads, so a host answering with a page of HTML would otherwise be quoted in full.
const ERROR_MAX_BYTES: u64 = 8 * 1024;

/// Ceiling on a success body. The widest is a bulk lookup of [`MAX_LOOKUPS_PER_POST`] recordings.
const ANSWER_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Identifies Melodia as the player and submitting client in listen metadata.
const MEDIA_PLAYER: &str = "Melodia";
const SUBMISSION_CLIENT: &str = "Melodia";
const SUBMISSION_CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Server cap on `POST /1/metadata/lookup/` — the bulk MBID lookup batch size.
pub const MAX_LOOKUPS_PER_POST: usize = 50;

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
    let bytes = crate::services::net::read_capped(
        response,
        "ListenBrainz validate-token",
        ANSWER_MAX_BYTES,
    )
    .await?;
    let validated: ValidatedToken = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::network("Failed to parse ListenBrainz validate-token response", e)
    })?;
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

/// Love or clear feedback for a recording via `POST /1/feedback/recording-feedback`.
/// `score` is `1` to love, `0` to clear (unlove). Only meaningful for a track
/// with a known `recording_mbid` — LB feedback keys on it — so the caller gates
/// on that. Durable/retried like a listen, sharing the same error policy.
pub async fn submit_feedback(
    client: &reqwest::Client,
    token: &str,
    recording_mbid: &str,
    score: i8,
) -> Result<(), ListenBrainzError> {
    let response = client
        .post(format!("{LB_API_BASE}/1/feedback/recording-feedback"))
        .header(reqwest::header::AUTHORIZATION, format!("Token {token}"))
        .json(&feedback_payload(recording_mbid, score))
        .send()
        .await
        .map_err(|e| AppError::network("ListenBrainz recording-feedback request failed", e))?;
    classify_response(response).await
}

/// A resolved `MusicBrainz` mapping for one track, from `metadata/lookup`.
#[derive(Debug, Clone)]
pub struct MbidMatch {
    pub recording_mbid: String,
    pub release_mbid: Option<String>,
}

/// One recording to resolve: artist + title, with an optional release to
/// sharpen the match.
pub struct LookupQuery<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub release: Option<&'a str>,
}

/// Resolve up to [`MAX_LOOKUPS_PER_POST`] recordings in one `POST
/// /1/metadata/lookup/`. Returns a vec aligned 1:1 with `queries` (unmatched
/// slots are `None`), keyed off each result's echoed `index` so a reordered or
/// short response can't misalign a mapping onto the wrong track.
pub async fn lookup_recording_mbids_bulk(
    client: &reqwest::Client,
    token: &str,
    queries: &[LookupQuery<'_>],
) -> Result<Vec<Option<MbidMatch>>, ListenBrainzError> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    debug_assert!(queries.len() <= MAX_LOOKUPS_PER_POST);
    let response = client
        .post(format!("{LB_API_BASE}/1/metadata/lookup/"))
        .header(reqwest::header::AUTHORIZATION, format!("Token {token}"))
        .json(&bulk_lookup_payload(queries))
        .send()
        .await
        .map_err(|e| AppError::network("ListenBrainz bulk metadata-lookup request failed", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(error_for(status, response).await);
    }
    let bytes = crate::services::net::read_capped(
        response,
        "ListenBrainz bulk metadata-lookup",
        ANSWER_MAX_BYTES,
    )
    .await?;
    let results: Vec<BulkLookupResult> = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::network("Failed to parse ListenBrainz bulk metadata-lookup response", e)
    })?;
    Ok(align_bulk_results(queries.len(), results))
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
    classify_response(response).await
}

/// Turn a completed response into `Ok(())` on success, else the classified error
/// (rate-limit / invalid-token / server). Shared by listen submission and
/// recording feedback.
async fn classify_response(response: reqwest::Response) -> Result<(), ListenBrainzError> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(error_for(status, response).await)
}

/// Classify a **non-success** response into its `ListenBrainzError`. Split out of
/// [`classify_response`] so the lookup endpoints — which must parse the body on
/// success — reuse the exact 429 / 401 / server policy.
async fn error_for(status: StatusCode, response: reqwest::Response) -> ListenBrainzError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ListenBrainzError::RateLimited {
            reset_in_secs: rate_limit_reset_in(response.headers()),
        };
    }
    if status == StatusCode::UNAUTHORIZED {
        return ListenBrainzError::InvalidToken;
    }
    server_error(status, response).await
}

/// Read a failed response's body into a `Server` error — best-effort, so an
/// unreadable body, or one past [`ERROR_MAX_BYTES`], degrades to an empty
/// message.
async fn server_error(status: StatusCode, response: reqwest::Response) -> ListenBrainzError {
    let message =
        crate::services::net::read_capped(response, "ListenBrainz error body", ERROR_MAX_BYTES)
            .await
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
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

/// Fallback wait when a 429 carries no `X-RateLimit-Reset-In` header to honor.
const DEFAULT_RATE_LIMIT_BACKOFF_SECS: u64 = 30;
/// Cap on a single rate-limit wait so a misbehaving header can't park a task for
/// minutes; a longer real window just costs one extra retry.
const MAX_RATE_LIMIT_BACKOFF_SECS: u64 = 300;

/// The backoff to honor for a rate-limited (`429`) response: the reported
/// seconds-until-reset (`X-RateLimit-Reset-In`, already parsed) if present, else
/// [`DEFAULT_RATE_LIMIT_BACKOFF_SECS`], clamped to [`MAX_RATE_LIMIT_BACKOFF_SECS`].
/// Shared by the scrobble submitter and the MBID backfill so both honor the same
/// policy.
pub fn rate_limit_backoff(reset_in_secs: Option<u64>) -> Duration {
    Duration::from_secs(
        reset_in_secs.unwrap_or(DEFAULT_RATE_LIMIT_BACKOFF_SECS).min(MAX_RATE_LIMIT_BACKOFF_SECS),
    )
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
        listen_type: if listens.len() == 1 {
            "single"
        } else {
            "import"
        },
        payload: listens
            .iter()
            .map(|(track, timestamp)| Listen {
                listened_at: Some(*timestamp),
                track_metadata: build_metadata(track),
            })
            .collect(),
    }
}

/// The `recording-feedback` body: the recording MBID and its `1`/`0` score.
fn feedback_payload(recording_mbid: &str, score: i8) -> RecordingFeedback<'_> {
    RecordingFeedback {
        recording_mbid,
        score,
    }
}

/// The bulk `metadata/lookup` body: one `recordings` entry per query.
fn bulk_lookup_payload<'a>(queries: &'a [LookupQuery<'a>]) -> BulkLookupRequest<'a> {
    BulkLookupRequest {
        recordings: queries
            .iter()
            .map(|q| LookupRecording {
                artist: q.artist,
                recording: q.title,
                release: q.release,
            })
            .collect(),
    }
}

/// Fold a bulk lookup response back into an input-aligned vec. Each result
/// carries the `index` it was requested at, so out-of-order or partial responses
/// still land on the right track; unmatched inputs stay `None`.
fn align_bulk_results(len: usize, results: Vec<BulkLookupResult>) -> Vec<Option<MbidMatch>> {
    let mut out: Vec<Option<MbidMatch>> = vec![None; len];
    for result in results {
        let index = result.index;
        if let Some(matched) = mbid_match(result.recording_mbid, result.release_mbid)
            && let Some(slot) = out.get_mut(index)
        {
            *slot = Some(matched);
        }
    }
    out
}

/// Build an [`MbidMatch`] iff a non-empty `recording_mbid` is present — the one
/// field LB feedback keys on. A blank or absent recording id is treated as "no
/// match".
fn mbid_match(recording_mbid: Option<String>, release_mbid: Option<String>) -> Option<MbidMatch> {
    let recording_mbid = recording_mbid.filter(|s| !s.trim().is_empty())?;
    Some(MbidMatch {
        recording_mbid,
        release_mbid: release_mbid.filter(|s| !s.trim().is_empty()),
    })
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
struct RecordingFeedback<'a> {
    recording_mbid: &'a str,
    score: i8,
}

#[derive(Serialize)]
struct BulkLookupRequest<'a> {
    recordings: Vec<LookupRecording<'a>>,
}

#[derive(Serialize)]
struct LookupRecording<'a> {
    #[serde(rename = "artist_name")]
    artist: &'a str,
    #[serde(rename = "recording_name")]
    recording: &'a str,
    #[serde(rename = "release_name", skip_serializing_if = "Option::is_none")]
    release: Option<&'a str>,
}

/// One entry of a `POST /1/metadata/lookup/` array. `index` echoes the input
/// position; unmatched entries carry only the echo fields, so both mbids are
/// `#[serde(default)]`.
#[derive(Deserialize)]
struct BulkLookupResult {
    index: usize,
    #[serde(default)]
    recording_mbid: Option<String>,
    #[serde(default)]
    release_mbid: Option<String>,
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
