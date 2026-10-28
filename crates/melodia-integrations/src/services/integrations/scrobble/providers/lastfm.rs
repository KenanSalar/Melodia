//! The signed Last.fm calls: desktop auth, now-playing, scrobbles and love/unlove.
//!
//! The key and shared secret identify the application, not an account, and are read at compile
//! time so nothing secret is committed. A keyless build leaves `ListenBrainz` working and this
//! surface inert ([`is_configured`]). Last.fm reports a failure in the body under HTTP 200, so
//! the in-body code is what becomes a [`LastfmError`].

use std::collections::BTreeMap;

use serde::Serialize;

use crate::services::integrations::scrobble::credentials::LastfmCredentials;
use crate::services::integrations::scrobble::model::ScrobbleTrack;
use melodia_core::error::AppError;

const LASTFM_API_KEY: Option<&str> = non_empty_env(option_env!("LASTFM_API_KEY"));
const LASTFM_SHARED_SECRET: Option<&str> = non_empty_env(option_env!("LASTFM_SHARED_SECRET"));

/// CI sets `LASTFM_API_KEY: ${{ secrets.LASTFM_API_KEY }}` unconditionally, and a missing secret
/// arrives as `Some("")`, which has to read as keyless.
const fn non_empty_env(value: Option<&str>) -> Option<&str> {
    match value {
        Some(s) if s.is_empty() => None,
        other => other,
    }
}

/// A constant, unlike [`listenbrainz::LB_API_BASE`]: a POST needs the compile-time keys, so a
/// drain test against it would run on a keyed build and skip on keyless CI. The retry policy is
/// what's testable either way, and `submit::lastfm_reaction` holds it with no socket.
///
/// [`listenbrainz::LB_API_BASE`]: super::listenbrainz::LB_API_BASE
const LASTFM_ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";

/// Ceiling on a response body. A scrobble answer is a few hundred bytes; the slack is for the
/// error shape, which carries a message written for a human.
const RESPONSE_MAX_BYTES: u64 = 256 * 1024;

/// Whether this build shipped both app keys.
pub fn is_configured() -> bool {
    keys().is_some()
}

/// `None` unless the build shipped both, every signed call taking the pair.
pub fn keys() -> Option<(&'static str, &'static str)> {
    Some((LASTFM_API_KEY?, LASTFM_SHARED_SECRET?))
}

/// Classified by the in-body code, not the HTTP status, for the submitter's retry policy.
#[derive(Debug, thiserror::Error)]
pub enum LastfmError {
    /// Error 9: the session key is no longer valid.
    #[error("Last.fm session key rejected (error 9)")]
    InvalidSession,
    /// Errors 11 and 16 (service offline or unavailable) and 29 (rate limit).
    #[error("Last.fm temporarily unavailable (error {code}): {message}")]
    Transient { code: u32, message: String },
    /// Any other code, retried all the same (`submit::lastfm_reaction`).
    #[error("Last.fm API error {code}: {message}")]
    Api { code: u32, message: String },
    /// Transport or response-decode failure (no reply, malformed JSON, …).
    #[error(transparent)]
    Transport(#[from] AppError),
}

/// Desktop auth, step one: a token for the user to approve on Last.fm's page.
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

/// Desktop auth, step two: the approved token traded for the session to persist.
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
    Ok(LastfmCredentials { session_key: session_key.to_owned(), username: username.to_owned() })
}

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

/// The caller chunks to Last.fm's cap of 50 listens a POST. An empty batch is a no-op.
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

/// Mirrors the in-app favorite onto the user's account.
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

fn classify(code: u32, message: String) -> LastfmError {
    match code {
        9 => LastfmError::InvalidSession,
        11 | 16 | 29 => LastfmError::Transient { code, message },
        _ => LastfmError::Api { code, message },
    }
}

/// The `api_sig`: MD5 hex of `name1value1name2value2…` in name order, the shared secret
/// appended. `format` and `api_sig` stay out of it by being added only after signing.
fn sign<K: AsRef<str>>(params: &BTreeMap<K, String>, secret: &str) -> String {
    let mut signing_input = String::new();
    for (name, value) in params {
        signing_input.push_str(name.as_ref());
        signing_input.push_str(value);
    }
    signing_input.push_str(secret);
    format!("{:x}", md5::compute(signing_input))
}

fn finalize_and_sign<K>(mut params: BTreeMap<K, String>, secret: &str) -> BTreeMap<K, String>
where
    K: Ord + AsRef<str> + From<&'static str>,
{
    let api_sig = sign(&params, secret);
    params.insert(K::from("api_sig"), api_sig);
    params.insert(K::from("format"), "json".to_owned());
    params
}

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
        melodia_net::services::net::read_capped(response, "Last.fm response", RESPONSE_MAX_BYTES)
            .await?;
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
