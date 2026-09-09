//! lrclib.net, the lyrics directory.
//!
//! At most two requests per track change, and only from behind `library::lyrics`' switch. The
//! service's own response shape stays private here and what crosses out is
//! [`LyricsAnswer`], the same way the radio directory keeps its `ApiStation` to itself.
//!
//! **Every request goes through the pacer, and a refusal arms it.** The service documents both
//! halves as requirements rather than advice: a delay between requests, and a `429` whose
//! `Retry-After` a client *must* honour on pain of being blocked by `User-Agent`. Sending again
//! into a window we have been told is closed is the specific behaviour its maintainer has named
//! as what turns a busy hour into an outage.

use std::time::Duration;

use serde::Deserialize;

use melodia_core::entities::lyrics::{LyricsAnswer, carries_gloss};
use melodia_core::error::AppError;

use super::LookupError;
use super::recording::{Recording, query_artist, query_title};
use crate::services::net::pacer::{RequestPacer, Turn};

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

/// How long to stand down after a refusal that named no period.
///
/// A refusal with no usable header still has to cost something, or the next track change walks
/// straight back into the closed window.
const DEFAULT_BACKOFF: Duration = Duration::from_mins(1);

/// The longest a single refusal may hold the lookup off.
///
/// A header claiming an implausible window would otherwise park the feature for the rest of the
/// session; capping it costs one extra refusal where the window really was that long.
const MAX_BACKOFF: Duration = Duration::from_mins(15);

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
    /// The index's own idea of what this row is, which is the half that has to be checked: it is
    /// a keyword search, and it will answer with another artist's song of the same name.
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    album_name: String,
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
    ///
    /// [`LyricsAnswer::is_synced`] asks it of a converted answer and cannot be reached from here:
    /// the row is filtered while it still carries the duration and album the ranking below needs.
    fn is_timed(&self) -> bool {
        self.synced_lyrics.as_deref().is_some_and(|text| !text.trim().is_empty())
    }

    /// How far this row's recording sits from the one playing, in milliseconds.
    ///
    /// Fractional because [`DURATION_TOLERANCE_MS`] is measured against it, and because it is the
    /// last separator the ranking has left once [`Self::second_off`] and the two ranks under it
    /// have all tied.
    fn distance_ms(&self, duration_ms: i64) -> f64 {
        let track_ms = f64::from(i32::try_from(duration_ms).unwrap_or(i32::MAX));
        (self.duration.max(0.0) * 1000.0 - track_ms).abs()
    }

    /// [`Self::distance_ms`] in whole seconds, which is the resolution the choice is made at.
    ///
    /// The directory files a length per upload and a tag carries one per file, and neither is
    /// measured against the other; below a second the difference between two rows says nothing
    /// about which is the better sheet. Saturates rather than wrapping, and the filter has already
    /// bounded what reaches it.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "non-negative by `abs`, and bounded by `DURATION_TOLERANCE_MS` before it is read"
    )]
    fn second_off(&self, duration_ms: i64) -> u32 {
        (self.distance_ms(duration_ms) / 1000.0) as u32
    }

    /// What this row claims to be, for [`Recording::matches`] to check.
    fn recording(&self) -> Recording {
        Recording::new(&self.track_name, &self.artist_name)
    }
}

/// Asks the directory about one track.
///
/// The signature first, and the index only for what it could not answer: a timed sheet, or a
/// glossed one. So an ordinary hit still costs one request, and the second is spent exactly where
/// the first left the panel something a reader cannot use.
///
/// `Ok(None)` is a miss rather than a failure, which is what a `404` means here and the majority
/// of what this function returns for an ordinary library. Every other non-success status is an
/// error, so an outage reads as one instead of as a library nobody has written lyrics for.
pub(super) async fn fetch(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    artist: &str,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, LookupError> {
    let exact = get_exact(client, pacer, title, artist, album, duration_ms).await?;
    if exact.as_ref().is_some_and(|answer| answer.instrumental || settles_it(answer)) {
        return Ok(exact);
    }
    // Past `settles_it`, an answer already timed leaves the index one errand: the gloss.
    let gloss_errand = exact.as_ref().is_some_and(LyricsAnswer::is_synced);

    match search_timed(client, pacer, title, artist, album, duration_ms).await {
        Ok(Some(timed)) if worth_taking(exact.as_ref(), &timed) => Ok(Some(timed)),
        Ok(Some(_) | None) => Ok(exact),
        // **A failed errand for a gloss may not cost the sheet it was run for.** Reported as a
        // failure it reaches the panel as "could not ask", which clears a sheet the reader could
        // already follow and stores nothing, so the next play pays for both requests again.
        Err(_) if gloss_errand => Ok(exact),
        // **A refusal ends the lookup whatever the signature found.** The index request would be
        // one more knock on a door we have just been told is shut, and the pacer would refuse it
        // anyway; reporting it is what lets the caller say so rather than claim a miss.
        Err(e @ LookupError::RateLimited { .. }) => Err(e),
        // **Otherwise only fatal where it was the only source left.** With a sheet already in hand
        // the failure costs its timings; with none, reporting a miss would have the caller record
        // "nobody has this" for a month on the strength of an outage.
        Err(e) if exact.is_some() => {
            log::debug!("lyrics: index unreachable: {e}");
            Ok(exact)
        }
        Err(e) => Err(e),
    }
}

/// Whether the signature's answer leaves the index nothing to add.
///
/// **A timed sheet used to settle it on its own, and the duration is why that was not enough.**
/// The directory files several uploads per recording and only some gloss their lines, so which one
/// the signature returns comes down to how the track's length rounded to the second — a track half
/// a second either side of a boundary gets a different upload, and the plain one is as likely as
/// not. Asking the index costs a request and buys the gloss back.
///
/// **Gated on `is_ascii`**, the same cheap question the resolver asks before it romanizes, and
/// deliberately the loose one: a gloss is a translation *out of* the language sung, so an English
/// sheet wants none, but an accented Latin one is not ASCII and pays for a request it will usually
/// spend for nothing. That is the direction to fail in. The precise predicate is
/// `romanize::needs_romanization`, which is a crate away and cannot be reached from here.
fn settles_it(answer: &LyricsAnswer) -> bool {
    // An untimed sheet leaves the index its original errand, whatever else it carries.
    if !answer.is_synced() {
        return false;
    }
    answer.has_gloss() || answer.text().is_some_and(str::is_ascii)
}

/// Whether the index's row is worth taking over what the signature answered.
///
/// **A timed sheet beats no sheet and an untimed one**, which is what the index was added for.
/// Against a sheet that is *already* timed the bar is the gloss: both are followable, and swapping
/// one for the other on anything less would trade a length the signature matched for one merely
/// inside the window.
fn worth_taking(exact: Option<&LyricsAnswer>, found: &LyricsAnswer) -> bool {
    match exact {
        Some(answer) if answer.is_synced() => found.has_gloss(),
        _ => true,
    }
}

/// The row whose four fields are this recording's, or `None` where the directory has no such row.
async fn get_exact(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    artist: &str,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, LookupError> {
    let mut url = endpoint(GET_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("track_name", title)
        .append_pair("artist_name", artist)
        .append_pair("album_name", album)
        // Rounded rather than truncated: the window either side is two seconds, and a track
        // reported a second short spends half of it before the request is even made.
        .append_pair("duration", &((duration_ms + 500) / 1000).to_string());

    let Some(body) = send(client, pacer, url, "Lyrics sheet", MAX_BYTES).await? else {
        return Ok(None);
    };
    let answer: ApiLyrics = serde_json::from_slice(&body)
        .map_err(|e| failed("Failed to parse the lyrics response", e))?;

    Ok(Some(answer.into_answer()))
}

/// The index's best timed row for this recording.
///
/// **Asked loosely and answered strictly**, which is the split the two endpoints are for. The
/// query drops the `feat.` credit no two taggers spell alike and the guests after the first,
/// because a row filed under one of them is still this song; the album goes too, being the tag
/// most likely to disagree with whoever uploaded the sheet and the one the signature already
/// tried. Then every row that comes back is checked against the tags in hand — the index is a
/// keyword search, so "Heartbeat" by an artist it half-matched is exactly what it is built to
/// return.
///
/// Three filters, in the order that makes each cheap: a row without timings is nothing this call
/// was made for, a row outside the duration window is a different recording, and a row naming a
/// different song is what [`Recording::matches`] is for. What survives ranks by the whole second
/// it is out by, then by whether it glosses its lines, then by the album.
async fn search_timed(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    artist: &str,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, LookupError> {
    // **Asked here first, because nothing this side cannot compare is worth a request.** A title
    // that is nothing but a bracketed credit, or a credit that is nothing but separators, folds to
    // a recording `Recording::matches` refuses every row against, so the cap could only be spent
    // to be told nothing.
    let ours = Recording::new(title, artist);
    if !ours.can_match() {
        return Ok(None);
    }

    // Back to the tag where the trim empties a field. The cut is there to widen the query, and a
    // title opening on its own `feat.` would otherwise be searched for by nothing at all.
    let track_query = match query_title(title) {
        "" => title,
        cut => cut,
    };
    let artist_query = match query_artist(artist) {
        "" => artist,
        cut => cut,
    };

    let mut url = endpoint(SEARCH_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("track_name", track_query)
        .append_pair("artist_name", artist_query);

    let Some(body) = send(client, pacer, url, "Lyrics search", MAX_SEARCH_BYTES).await? else {
        return Ok(None);
    };
    let rows: Vec<ApiLyrics> = serde_json::from_slice(&body)
        .map_err(|e| failed("Failed to parse the lyrics search response", e))?;

    Ok(pick_timed(rows, &ours, album, duration_ms).map(ApiLyrics::into_answer))
}

/// The row the index returned that is this recording, or nothing it can stand behind.
///
/// Split out so the tolerance and the tie-break are table-testable: everything either side of it
/// needs a socket, and this is where the answer is actually decided.
///
/// The filter runs cheapest predicate first, and `min_by` keeps the first of equal rows, so a full
/// tie falls back to the order the index sent.
fn pick_timed(
    rows: Vec<ApiLyrics>,
    ours: &Recording,
    album: &str,
    duration_ms: i64,
) -> Option<ApiLyrics> {
    let album = melodia_core::utils::fold::fold(album);

    rows.into_iter()
        .filter(|row| {
            row.is_timed()
                && row.distance_ms(duration_ms) <= DURATION_TOLERANCE_MS
                && ours.matches(&row.recording())
        })
        .min_by(|a, b| {
            a.second_off(duration_ms)
                .cmp(&b.second_off(duration_ms))
                .then_with(|| gloss_rank(a).cmp(&gloss_rank(b)))
                .then_with(|| album_rank(a, &album).cmp(&album_rank(b, &album)))
                .then_with(|| a.distance_ms(duration_ms).total_cmp(&b.distance_ms(duration_ms)))
        })
}

/// `0` where the row glosses its lines, `1` otherwise.
///
/// **Ranked above the album and under the second**, which is the order the three answer in. Two
/// uploads whose lengths agree to the second are the same recording twice and the choice between
/// them is what they carry, so a fifth of a second must not decide it — that is noise, and it is
/// exactly what handed a reader the one upload of six with no translation in it. A row a whole
/// second further out is a different matter and still loses.
fn gloss_rank(row: &ApiLyrics) -> u8 {
    u8::from(!row.synced_lyrics.as_deref().is_some_and(carries_gloss))
}

/// `0` where the row is filed under the album in hand, `1` otherwise. A tie-break and never more
/// than that: two rows the same distance from the track's duration are the same recording twice,
/// and the release it was ripped from is the only thing left to prefer.
fn album_rank(row: &ApiLyrics, album: &str) -> u8 {
    u8::from(album.is_empty() || melodia_core::utils::fold::fold(&row.album_name) != *album)
}

/// One GET under a cap and behind the pacer, with `None` for the `404` both endpoints spell a miss
/// as.
///
/// **The single choke point, which is why the classification is here.** Both endpoints reach the
/// network through this one function, so the floor, the stop and the reading of a refusal are each
/// written once and cannot be half-applied by a new call site.
async fn send(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    url: reqwest::Url,
    what: &str,
    cap: u64,
) -> Result<Option<Vec<u8>>, LookupError> {
    if let Turn::Stopped(left) = pacer.acquire().await {
        return Err(LookupError::RateLimited {
            retry_after: Some(left),
        });
    }

    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| failed("Lyrics lookup failed", e))?;

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    // **Both refusals, and read off the status rather than the body.** The service's own
    // documentation of the `429` payload does not match what it sends, and the `503` it sheds load
    // with is documented nowhere at all; both carry a `Retry-After` and both mean the same thing to
    // a client, which is to come back later.
    if matches!(
        status,
        reqwest::StatusCode::TOO_MANY_REQUESTS | reqwest::StatusCode::SERVICE_UNAVAILABLE
    ) {
        let retry_after = retry_after(response.headers());
        pacer.stop_for(retry_after.unwrap_or(DEFAULT_BACKOFF)).await;
        return Err(LookupError::RateLimited { retry_after });
    }
    if !status.is_success() {
        // A `520` is Cloudflare's, not the directory's, and it is the shape a blocked `User-Agent`
        // gets back. Named here because nothing else in the tree would explain it and the answer
        // is not something a retry reaches.
        if status.as_u16() == 520 {
            log::warn!(
                "lyrics: the directory's edge refused us outright (HTTP 520), which is what a \
                 blocked client sees"
            );
        }
        return Err(LookupError::Failed(AppError::network_msg(format!(
            "Lyrics lookup returned HTTP {}",
            status.as_u16()
        ))));
    }

    crate::services::net::read_capped(response, what, cap)
        .await
        .map(Some)
        .map_err(LookupError::Failed)
}

/// The `Retry-After` a refusal carried, clamped to something a session can sit out.
///
/// **Delta-seconds only.** The header's other spelling is an HTTP-date, and parsing one would buy
/// a date crate for this crate to read a form rate limiters do not send; an unreadable value falls
/// through to the caller's default, which is the same answer as an absent one.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let secs: u64 = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())?;

    Some(Duration::from_secs(secs).min(MAX_BACKOFF))
}

/// One of the two endpoint constants above, ready for its query pairs.
///
/// Both are literals that parse, so the error arm is unreachable rather than merely unlikely —
/// which is why it costs one spelling between them rather than one each.
fn endpoint(url: &'static str) -> Result<reqwest::Url, LookupError> {
    reqwest::Url::parse(url).map_err(|e| failed("Lyrics directory endpoint is not a URL", e))
}

/// An I/O-boundary failure, which is every arm of this module that is not a refusal.
fn failed(
    msg: &'static str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> LookupError {
    LookupError::Failed(AppError::network(msg, source))
}

#[cfg(test)]
#[path = "tests/lrclib_tests.rs"]
mod tests;
