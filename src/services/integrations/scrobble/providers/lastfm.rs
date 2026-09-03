//! Last.fm provider: the signed API calls behind desktop auth, "now playing",
//! scrobble submission, and love/unlove.
//!
//! The API key + shared secret identify the *application*, not a listening
//! account — every user scrobbles to their own account via their own session
//! key. They're read at compile time from the environment so nothing secret is
//! committed: official releases inject them as CI secrets, while a keyless local
//! build leaves `ListenBrainz` fully working and the Last.fm surface inert
//! (gated on [`is_configured`]).
//!
//! Every call POSTs to the 2.0 endpoint as form-urlencoded with `format=json`.
//! Last.fm reports failures in the response *body* (HTTP stays 200), so the
//! functions parse the in-body `{"error": <code>}` into a [`LastfmError`] the
//! submitter can act on.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::error::AppError;
use crate::services::integrations::scrobble::credentials::LastfmCredentials;
use crate::services::integrations::scrobble::model::ScrobbleTrack;

pub const LASTFM_API_KEY: Option<&str> = non_empty_env(option_env!("LASTFM_API_KEY"));
pub const LASTFM_SHARED_SECRET: Option<&str> = non_empty_env(option_env!("LASTFM_SHARED_SECRET"));

/// Treat a present-but-empty compile-time env var as absent. CI wires
/// `env: LASTFM_API_KEY: ${{ secrets.LASTFM_API_KEY }}` unconditionally, and a
/// missing secret substitutes an empty string that `option_env!` reports as
/// `Some("")`, not `None` — folding it back to `None` keeps a keyless build
/// ListenBrainz-only.
const fn non_empty_env(value: Option<&str>) -> Option<&str> {
    match value {
        Some(s) if s.is_empty() => None,
        other => other,
    }
}

/// The Last.fm 2.0 API endpoint. Every method POSTs here as form-urlencoded.
const LASTFM_ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";

/// Ceiling on a response body. A scrobble answer is a few hundred bytes; the slack is for the
/// error shape, which carries a message written for a human.
const RESPONSE_MAX_BYTES: u64 = 256 * 1024;

/// Whether this build shipped Last.fm app credentials (both key and secret).
/// Gates the Last.fm setter / UI / detector so a keyless build ships
/// ListenBrainz-only.
pub fn is_configured() -> bool {
    LASTFM_API_KEY.is_some() && LASTFM_SHARED_SECRET.is_some()
}

/// A Last.fm API call's outcome, classified so the submitter can pick a policy.
/// The retry decision hinges on the numeric in-body error code, not HTTP status.
#[derive(Debug, thiserror::Error)]
pub enum LastfmError {
    /// Error 9 — the session key is no longer valid. Drop it, prompt a re-auth,
    /// and stop retrying.
    #[error("Last.fm session key rejected (error 9)")]
    InvalidSession,
    /// Errors 11/16 (service offline / temporarily unavailable) and 29 (rate
    /// limit): keep the work queued and back off.
    #[error("Last.fm temporarily unavailable (error {code}): {message}")]
    Transient { code: u32, message: String },
    /// Any other API-level error code. Retried with the submitter's exponential
    /// backoff; there's no per-item attempt counter, so a genuinely permanent
    /// code (e.g. 10 invalid API key, 26 suspended) keeps its queue slot until the
    /// cap evicts it. Kept queued deliberately — a code mis-classified as
    /// permanent would otherwise silently lose the listen.
    #[error("Last.fm API error {code}: {message}")]
    Api { code: u32, message: String },
    /// Transport or response-decode failure (no reply, malformed JSON, …).
    #[error(transparent)]
    Transport(#[from] AppError),
}

/// Step 1 of desktop auth: request an unauthorized token to send the user to the
/// Last.fm approval page. Signed, but carries no session key yet.
pub async fn get_token(
    client: &reqwest::Client,
    api_key: &str,
    secret: &str,
) -> Result<String, LastfmError> {
    let mut params = BTreeMap::new();
    params.insert("method", "auth.getToken".to_owned());
    params.insert("api_key", api_key.to_owned());
    let params = finalize_and_sign(params, secret);

    let body = send(client, &params).await?;
    let token = body
        .get("token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::network_msg("Last.fm auth.getToken: response had no token"))?;
    Ok(token.to_owned())
}

/// Step 2 of desktop auth: exchange the user-approved token for a long-lived
/// session key, returning the credentials to persist.
pub async fn get_session(
    client: &reqwest::Client,
    api_key: &str,
    secret: &str,
    token: &str,
) -> Result<LastfmCredentials, LastfmError> {
    let mut params = BTreeMap::new();
    params.insert("method", "auth.getSession".to_owned());
    params.insert("api_key", api_key.to_owned());
    params.insert("token", token.to_owned());
    let params = finalize_and_sign(params, secret);

    let body = send(client, &params).await?;
    let session = body
        .get("session")
        .ok_or_else(|| AppError::network_msg("Last.fm auth.getSession: response had no session"))?;
    let session_key = session
        .get("key")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::network_msg("Last.fm auth.getSession: session had no key"))?;
    let username = session
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::network_msg("Last.fm auth.getSession: session had no name"))?;
    Ok(LastfmCredentials {
        session_key: session_key.to_owned(),
        username: username.to_owned(),
    })
}

/// Report the currently-playing track. Ephemeral — never retried or queued.
pub async fn update_now_playing(
    client: &reqwest::Client,
    api_key: &str,
    secret: &str,
    session_key: &str,
    track: &ScrobbleTrack,
) -> Result<(), LastfmError> {
    let params = finalize_and_sign(now_playing_params(api_key, session_key, track), secret);
    send(client, &params).await?;
    Ok(())
}

/// Submit a batch of scrobbles. Last.fm caps a single POST at 50 listens; the
/// caller chunks accordingly. An empty batch is a no-op.
pub async fn scrobble_batch(
    client: &reqwest::Client,
    api_key: &str,
    secret: &str,
    session_key: &str,
    batch: &[(&ScrobbleTrack, i64)],
) -> Result<(), LastfmError> {
    if batch.is_empty() {
        return Ok(());
    }
    let params = finalize_and_sign(scrobble_params(api_key, session_key, batch), secret);
    send(client, &params).await?;
    Ok(())
}

/// Love or unlove a track on the user's account (mirrors the in-app favorite).
pub async fn love(
    client: &reqwest::Client,
    api_key: &str,
    secret: &str,
    session_key: &str,
    track: &ScrobbleTrack,
    loved: bool,
) -> Result<(), LastfmError> {
    let params = finalize_and_sign(love_params(api_key, session_key, track, loved), secret);
    send(client, &params).await?;
    Ok(())
}

/// Map a Last.fm in-body error code to its retry policy.
fn classify(code: u32, message: String) -> LastfmError {
    match code {
        9 => LastfmError::InvalidSession,
        11 | 16 | 29 => LastfmError::Transient { code, message },
        _ => LastfmError::Api { code, message },
    }
}

/// Compute the `api_sig`: MD5 hex of the params sorted by name and concatenated
/// as `name1value1name2value2…` with the shared secret appended.
///
/// `format` and `api_sig` are excluded from the signature simply by never being
/// in `params` — they're added to the request only after signing. `method` is
/// included; the `BTreeMap` supplies the required alphabetical key order.
fn sign<K: AsRef<str>>(params: &BTreeMap<K, String>, secret: &str) -> String {
    let mut signing_input = String::new();
    for (name, value) in params {
        signing_input.push_str(name.as_ref());
        signing_input.push_str(value);
    }
    signing_input.push_str(secret);
    format!("{:x}", md5::compute(signing_input))
}

/// Sign the request, then append the two params excluded from the signature:
/// `api_sig` itself and `format=json`.
fn finalize_and_sign<K>(mut params: BTreeMap<K, String>, secret: &str) -> BTreeMap<K, String>
where
    K: Ord + AsRef<str> + From<&'static str>,
{
    let api_sig = sign(&params, secret);
    params.insert(K::from("api_sig"), api_sig);
    params.insert(K::from("format"), "json".to_owned());
    params
}

/// The signed-portion params for `track.updateNowPlaying` (no `format` /
/// `api_sig` — those are added after signing).
fn now_playing_params(
    api_key: &str,
    session_key: &str,
    track: &ScrobbleTrack,
) -> BTreeMap<&'static str, String> {
    let mut params = BTreeMap::new();
    params.insert("method", "track.updateNowPlaying".to_owned());
    params.insert("api_key", api_key.to_owned());
    params.insert("sk", session_key.to_owned());
    params.insert("artist", track.artist.clone());
    params.insert("track", track.track.clone());
    if let Some(album) = &track.album {
        params.insert("album", album.clone());
    }
    if let Some(album_artist) = &track.album_artist {
        params.insert("albumArtist", album_artist.clone());
    }
    if let Some(track_number) = track.track_number {
        params.insert("trackNumber", track_number.to_string());
    }
    if let Some(duration_secs) = track.duration_secs {
        params.insert("duration", duration_secs.to_string());
    }
    if let Some(mbid) = &track.recording_mbid {
        params.insert("mbid", mbid.clone());
    }
    params
}

/// The signed-portion params for a `track.scrobble` batch, one bracketed index
/// per listen (`artist[0]`, `track[0]`, `timestamp[0]`, …).
fn scrobble_params(
    api_key: &str,
    session_key: &str,
    batch: &[(&ScrobbleTrack, i64)],
) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    params.insert("method".to_owned(), "track.scrobble".to_owned());
    params.insert("api_key".to_owned(), api_key.to_owned());
    params.insert("sk".to_owned(), session_key.to_owned());
    for (index, (track, timestamp)) in batch.iter().enumerate() {
        params.insert(format!("artist[{index}]"), track.artist.clone());
        params.insert(format!("track[{index}]"), track.track.clone());
        params.insert(format!("timestamp[{index}]"), timestamp.to_string());
        if let Some(album) = &track.album {
            params.insert(format!("album[{index}]"), album.clone());
        }
        if let Some(album_artist) = &track.album_artist {
            params.insert(format!("albumArtist[{index}]"), album_artist.clone());
        }
        if let Some(track_number) = track.track_number {
            params.insert(format!("trackNumber[{index}]"), track_number.to_string());
        }
        if let Some(mbid) = &track.recording_mbid {
            params.insert(format!("mbid[{index}]"), mbid.clone());
        }
        if let Some(duration_secs) = track.duration_secs {
            params.insert(format!("duration[{index}]"), duration_secs.to_string());
        }
    }
    params
}

/// The signed-portion params for `track.love` / `track.unlove`.
fn love_params(
    api_key: &str,
    session_key: &str,
    track: &ScrobbleTrack,
    loved: bool,
) -> BTreeMap<&'static str, String> {
    let method = if loved { "track.love" } else { "track.unlove" };
    let mut params = BTreeMap::new();
    params.insert("method", method.to_owned());
    params.insert("api_key", api_key.to_owned());
    params.insert("sk", session_key.to_owned());
    params.insert("artist", track.artist.clone());
    params.insert("track", track.track.clone());
    params
}

/// POST the finalized (signed) params as form-urlencoded and turn the response
/// into a classified result. Last.fm answers with HTTP 200 and an in-body
/// `{"error": <code>, "message": …}` on failure, so success is read from the
/// body, not the status.
async fn send<K: Serialize>(
    client: &reqwest::Client,
    params: &BTreeMap<K, String>,
) -> Result<serde_json::Value, LastfmError> {
    let response = client
        .post(LASTFM_ENDPOINT)
        .form(params)
        .send()
        .await
        .map_err(|e| AppError::network("Last.fm request failed", e))?;
    let bytes =
        crate::services::net::read_capped(response, "Last.fm response", RESPONSE_MAX_BYTES).await?;
    let body: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::network("Failed to parse Last.fm response", e))?;
    if let Some(code) = body.get("error").and_then(serde_json::Value::as_u64) {
        let message =
            body.get("message").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
        return Err(classify(u32::try_from(code).unwrap_or(u32::MAX), message));
    }
    Ok(body)
}

#[cfg(test)]
#[path = "tests/lastfm_tests.rs"]
mod tests;
