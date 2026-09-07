//! lrclib.net, the lyrics directory.
//!
//! At most two requests per track change, and only from behind `library::lyrics`' switch. The
//! service's own response shape stays private here and what crosses out is
//! [`LyricsAnswer`], the same way the radio directory keeps its `ApiStation` to itself.

use std::time::Duration;

use serde::Deserialize;

use melodia_core::entities::lyrics::LyricsAnswer;
use melodia_core::error::AppError;

/// The exact-signature endpoint: four fields that together name one recording, so it answers or it
/// does not, and nothing here has to decide which of several rows is this track.
const GET_ENDPOINT: &str = "https://lrclib.net/api/get";

/// The index behind it, and **it reaches records the signature cannot**. That is the finding this
/// fallback exists for rather than a preference for fuzzy matching: a library's tags routinely
/// name a shorter artist credit than the row that carries the timings, so the signature lands on a
/// duplicate with only plain text — and there are rows the signature returns `404` for while the
/// index returns them under an exact match on all three strings.
const SEARCH_ENDPOINT: &str = "https://lrclib.net/api/search";

/// What the service asks clients to identify themselves as: a name, a version and somewhere to
/// complain. The shared client already sends `Melodia/<version>`; this adds the project URL, and
/// only on requests going to this host.
const AGENT: &str =
    concat!("Melodia/", env!("CARGO_PKG_VERSION"), " (https://github.com/KenanSalar/Melodia)");

/// A sheet is text and text is small. Generous against the longest song anyone has written, and
/// still a refusal for a body that is not one.
const MAX_BYTES: u64 = 256 * 1024;

/// The index answers with whole sheets, a score of them at a time.
const MAX_SEARCH_BYTES: u64 = 2 * 1024 * 1024;

/// How far a candidate's own duration may sit from the track's and still be the same recording.
///
/// The signature endpoint's own window, measured against it: a track it answers for at `n` is
/// still answered at `n ± 2` and refused at `n ± 3`. Taking anything wider here would let a radio
/// edit answer for the album cut, which is precisely the confusion the signature avoids.
const DURATION_TOLERANCE_MS: f64 = 2_000.0;

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
    /// Seconds, fractional. Only the index needs it, the signature having matched on it already.
    #[serde(default)]
    duration: f64,
}

impl ApiLyrics {
    fn into_answer(self) -> LyricsAnswer {
        LyricsAnswer {
            synced: self.synced_lyrics,
            plain: self.plain_lyrics,
            instrumental: self.instrumental,
        }
    }

    /// Whether this row carries timings, which is the only thing the index is asked for.
    fn is_timed(&self) -> bool {
        self.synced_lyrics.as_deref().is_some_and(|text| !text.trim().is_empty())
    }

    /// How far this row's recording sits from the one playing, in milliseconds.
    ///
    /// Kept in `f64` rather than rounded to whole seconds either side: the field is fractional,
    /// and two rows a fifth of a second apart are the ones this has to choose between.
    fn distance_ms(&self, duration_ms: i64) -> f64 {
        let track_ms = f64::from(i32::try_from(duration_ms).unwrap_or(i32::MAX));
        (self.duration.max(0.0) * 1000.0 - track_ms).abs()
    }
}

/// Asks the directory about one track.
///
/// The signature first, and the index only for what it could not answer: a timed sheet. So an
/// ordinary hit still costs one request, and the second is spent exactly where the first left the
/// panel with words it cannot follow.
///
/// `Ok(None)` is a miss rather than a failure, which is what a `404` means here and the majority
/// of what this function returns for an ordinary library. Every other non-success status is an
/// error, so an outage reads as one instead of as a library nobody has written lyrics for.
pub async fn fetch(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, AppError> {
    let exact = get_exact(client, title, artist, album, duration_ms).await?;
    if exact.as_ref().is_some_and(|answer| answer.synced.is_some() || answer.instrumental) {
        return Ok(exact);
    }

    match search_timed(client, title, artist, duration_ms).await {
        Ok(Some(timed)) => Ok(Some(timed)),
        Ok(None) => Ok(exact),
        // **Only fatal where it was the only source left.** With a sheet already in hand the
        // failure costs its timings; with none, reporting a miss would have the caller record
        // "nobody has this" for a month on the strength of an outage.
        Err(e) if exact.is_some() => {
            log::debug!("lyrics: index unreachable: {}", melodia_core::error::describe(&e));
            Ok(exact)
        }
        Err(e) => Err(e),
    }
}

/// The row whose four fields are this recording's, or `None` where the directory has no such row.
async fn get_exact(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, AppError> {
    let mut url = reqwest::Url::parse(GET_ENDPOINT)
        .map_err(|e| AppError::network("Lyrics directory endpoint is not a URL", e))?;
    url.query_pairs_mut()
        .append_pair("track_name", title)
        .append_pair("artist_name", artist)
        .append_pair("album_name", album)
        // Rounded rather than truncated: the window either side is two seconds, and a track
        // reported a second short spends half of it before the request is even made.
        .append_pair("duration", &((duration_ms + 500) / 1000).to_string());

    let Some(body) = send(client, url, "Lyrics sheet", MAX_BYTES).await? else {
        return Ok(None);
    };
    let answer: ApiLyrics = serde_json::from_slice(&body)
        .map_err(|e| AppError::network("Failed to parse the lyrics response", e))?;

    Ok(Some(answer.into_answer()))
}

/// The index's best timed row for this recording, by duration.
///
/// **Asked without the album and answered on duration alone**, which is the field that separates
/// two recordings of one song; the album is what the signature above already tried and is the tag
/// most likely to disagree with whoever uploaded the sheet. A row outside the tolerance is a
/// different recording, and a plain row is nothing this call was made for — the signature's own
/// answer is at least the row the directory considers canonical.
async fn search_timed(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, AppError> {
    let mut url = reqwest::Url::parse(SEARCH_ENDPOINT)
        .map_err(|e| AppError::network("Lyrics directory endpoint is not a URL", e))?;
    url.query_pairs_mut().append_pair("track_name", title).append_pair("artist_name", artist);

    let Some(body) = send(client, url, "Lyrics search", MAX_SEARCH_BYTES).await? else {
        return Ok(None);
    };
    let rows: Vec<ApiLyrics> = serde_json::from_slice(&body)
        .map_err(|e| AppError::network("Failed to parse the lyrics search response", e))?;

    Ok(rows
        .into_iter()
        .filter(|row| row.is_timed() && row.distance_ms(duration_ms) <= DURATION_TOLERANCE_MS)
        .min_by(|a, b| a.distance_ms(duration_ms).total_cmp(&b.distance_ms(duration_ms)))
        .map(ApiLyrics::into_answer))
}

/// One GET under a cap, with `None` for the `404` both endpoints spell a miss as.
async fn send(
    client: &reqwest::Client,
    url: reqwest::Url,
    what: &str,
    cap: u64,
) -> Result<Option<Vec<u8>>, AppError> {
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

    Ok(Some(crate::services::net::read_capped(response, what, cap).await?))
}
