//! lrclib.net, the lyrics directory.
//!
//! One request per track change at most, and only from behind `library::lyrics`' switch. The
//! service's own response shape stays private here and what crosses out is
//! [`LyricsAnswer`], the same way the radio directory keeps its `ApiStation` to itself.

use std::time::Duration;

use serde::Deserialize;

use melodia_core::entities::lyrics::LyricsAnswer;
use melodia_core::error::AppError;

/// `/api/get` rather than `/api/search`, and the difference is who does the deciding. The four
/// fields together identify one recording, so the service answers or it does not. A search
/// endpoint hands back near-matches and moves the burden of picking which one is *this* track
/// here, which is a scoring pass whose only job would be to rebuild the certainty this starts
/// with.
const ENDPOINT: &str = "https://lrclib.net/api/get";

/// What the service asks clients to identify themselves as: a name, a version and somewhere to
/// complain. The shared client already sends `Melodia/<version>`; this adds the project URL, and
/// only on requests going to this host.
const AGENT: &str =
    concat!("Melodia/", env!("CARGO_PKG_VERSION"), " (https://github.com/KenanSalar/Melodia)");

/// A sheet is text and text is small. Generous against the longest song anyone has written, and
/// still a refusal for a body that is not one.
const MAX_BYTES: u64 = 256 * 1024;

/// In line with the station logo's, the other fetch that must not hold a view open waiting.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The response, spelled as the service spells it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiLyrics {
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    instrumental: bool,
}

/// Asks the directory about one track.
///
/// `Ok(None)` is a miss rather than a failure, which is what a `404` means here and the majority
/// of what this function returns for an ordinary library. Every other non-success status is an
/// error, so an outage reads as one instead of as a library nobody has written lyrics for.
pub async fn fetch(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
    duration_secs: i64,
) -> Result<Option<LyricsAnswer>, AppError> {
    let mut url = reqwest::Url::parse(ENDPOINT)
        .map_err(|e| AppError::network("Lyrics directory endpoint is not a URL", e))?;
    url.query_pairs_mut()
        .append_pair("track_name", title)
        .append_pair("artist_name", artist)
        .append_pair("album_name", album)
        .append_pair("duration", &duration_secs.to_string());

    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::network("Lyrics lookup failed", e))?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::network_msg(format!(
            "Lyrics lookup returned HTTP {}",
            status.as_u16()
        )));
    }

    let body = crate::services::net::read_capped(response, "Lyrics sheet", MAX_BYTES).await?;
    let answer: ApiLyrics = serde_json::from_slice(&body)
        .map_err(|e| AppError::network("Failed to parse the lyrics response", e))?;

    Ok(Some(LyricsAnswer {
        synced: answer.synced_lyrics,
        plain: answer.plain_lyrics,
        instrumental: answer.instrumental,
    }))
}
