//! lrclib.net, the lyrics directory: at most two requests per track change and one per search a
//! person asks for, only from behind `library::lyrics`' switch, and its wire shape stays here and
//! in [`lyricsfile`].
//!
//! **Every request goes through the pacer, and a refusal arms it.** The service requires both: a
//! delay between requests, and honouring a `429`'s `Retry-After` on pain of a `User-Agent` ban.

use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::lyrics::{LyricsAnswer, LyricsCandidate, carries_gloss};
use melodia_core::error::{AppError, describe};
use melodia_core::utils::fold::fold;

use super::LookupError;
use super::lyricsfile;
use super::recording::{Recording, search_terms};
use crate::services::net::pacer::{RequestPacer, Turn};

/// The exact signature: four fields naming one recording, so it answers or it doesn't.
const GET_ENDPOINT: &str = "https://lrclib.net/api/get";

/// The index, which **reaches rows the signature can't**: tags routinely name a shorter credit
/// than the timed row carries, and some rows the signature `404`s come back here on an exact match.
const SEARCH_ENDPOINT: &str = "https://lrclib.net/api/search";

/// What the service asks clients to send: a name, a version and somewhere to complain. Only on
/// requests to this host.
const AGENT: &str =
    concat!("Melodia/", env!("CARGO_PKG_VERSION"), " (https://github.com/KenanSalar/Melodia)");

/// Generous against the longest song, and still a refusal for a body that isn't a sheet.
const MAX_BYTES: u64 = 256 * 1024;

/// The index answers with whole sheets, a score of them at a time.
const MAX_SEARCH_BYTES: u64 = 2 * 1024 * 1024;

/// The signature endpoint's own window, measured: answered at `n ± 2` s and refused at `n ± 3`.
/// Any wider and a radio edit would answer for the album cut.
const DURATION_TOLERANCE_MS: f64 = 2_000.0;

/// In line with the station logo's, the other fetch that must not hold a view open waiting.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// For a refusal naming no period, so the next track change doesn't walk straight back in.
const DEFAULT_BACKOFF: Duration = Duration::from_mins(1);

/// So an implausible `Retry-After` can't park the feature for the rest of the session.
const MAX_BACKOFF: Duration = Duration::from_mins(15);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiLyrics {
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    instrumental: bool,
    /// Whether `lyricsfile` times the words, which `syncedLyrics` never does.
    #[serde(default)]
    has_word_sync: bool,
    #[serde(default)]
    lyricsfile: Option<String>,
    /// Seconds, fractional, checked against [`DURATION_TOLERANCE_MS`] on both legs.
    #[serde(default)]
    duration: f64,
    /// What the index thinks this row is, which has to be checked: it is a keyword search.
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    album_name: String,
}

impl ApiLyrics {
    /// Converts the Lyricsfile only here, for a row that answers or is offered.
    fn into_answer(self) -> LyricsAnswer {
        LyricsAnswer {
            synced: self.word_timed_sheet().or(self.synced_lyrics),
            plain: self.plain_lyrics,
            instrumental: self.instrumental,
        }
    }

    /// The Lyricsfile as enhanced LRC, where it times the words.
    fn word_timed_sheet(&self) -> Option<String> {
        if !self.has_word_sync {
            return None;
        }
        self.lyricsfile.as_deref().and_then(lyricsfile::enhanced_lrc)
    }

    /// [`LyricsAnswer::is_synced`] on the raw row, which still carries what the ranking needs. A
    /// word-timed record carries its lines in `syncedLyrics` too.
    fn is_timed(&self) -> bool {
        self.synced_lyrics.as_deref().is_some_and(|text| !text.trim().is_empty())
    }

    fn is_glossed(&self) -> bool {
        self.synced_lyrics.as_deref().is_some_and(carries_gloss)
    }

    /// Fractional, being the ranking's last separator.
    fn distance_ms(&self, duration_ms: i64) -> f64 {
        let track_ms = f64::from(i32::try_from(duration_ms).unwrap_or(i32::MAX));
        (self.duration.max(0.0) * 1000.0 - track_ms).abs()
    }

    /// Whole seconds, the resolution the choice is made at: the directory files a length per
    /// upload and a tag one per file, so below a second two rows' lengths say nothing.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "non-negative by `abs`, and a float cast saturates, a row that far off sorting last"
    )]
    fn second_off(&self, duration_ms: i64) -> u32 {
        (self.distance_ms(duration_ms) / 1000.0) as u32
    }

    fn recording(&self) -> Recording {
        Recording::new(&self.track_name, &self.artist_name)
    }

    fn has_text(&self) -> bool {
        self.is_timed() || self.plain_lyrics.as_deref().is_some_and(|text| !text.trim().is_empty())
    }

    /// Words, then lines, then plain text, then nothing to read, which is what an instrumental
    /// row is. Words lit one by one are the most a sheet can carry.
    fn kind_rank(&self) -> u8 {
        if self.has_word_sync {
            0
        } else if self.is_timed() {
            1
        } else if self.has_text() {
            2
        } else {
            3
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped non-negative, and a float cast saturates past any song's length"
    )]
    fn into_candidate(mut self) -> LyricsCandidate {
        LyricsCandidate {
            title: std::mem::take(&mut self.track_name),
            artist: std::mem::take(&mut self.artist_name),
            album: std::mem::take(&mut self.album_name),
            duration_secs: self.duration.max(0.0).round() as u32,
            answer: self.into_answer(),
        }
    }
}

/// Asks the directory about one track: the signature first, and the index only where that left
/// no timed sheet, or one timing neither words nor a gloss, so an ordinary hit costs one request.
///
/// `Ok(None)` is a miss, which is what a `404` means and most of what a library gets. Every other
/// non-success status is an error, so an outage doesn't read as nobody having written lyrics.
pub(super) async fn fetch(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    credit: &ArtistCredit,
    album: &str,
    duration_ms: i64,
) -> Result<Option<LyricsAnswer>, LookupError> {
    let ours = Recording::from_credit(title, credit);
    let printed = credit.line().unwrap_or_default();
    let exact = get_exact(client, pacer, title, printed, duration_ms, &ours).await?;
    if exact.as_ref().is_some_and(|row| row.instrumental || settles_it(row)) {
        return Ok(exact.map(ApiLyrics::into_answer));
    }
    // Past `settles_it`, a row already timed leaves the index one errand: word times or a gloss.
    let timed_errand = exact.as_ref().is_some_and(ApiLyrics::is_timed);

    let chosen = match search_timed(client, pacer, title, credit, album, duration_ms, &ours).await {
        Ok(Some(timed)) if worth_taking(exact.as_ref(), &timed) => Some(timed),
        Ok(Some(_) | None) => exact,
        // A failed errand mustn't cost the sheet: as a failure it would clear what the reader
        // could already follow, and store nothing.
        Err(_) if timed_errand => exact,
        // A refusal ends the lookup whatever the signature found, so the caller can say so.
        Err(e @ LookupError::RateLimited { .. }) => return Err(e),
        // Otherwise fatal only with no sheet in hand, or the caller would record "nobody has
        // this" for a month on the strength of an outage.
        Err(e) if exact.is_some() => {
            log::debug!("lyrics: index unreachable: {}", describe(&e));
            exact
        }
        Err(e) => return Err(e),
    };
    Ok(chosen.map(ApiLyrics::into_answer))
}

/// Whether the signature's row leaves the index nothing to add. A timed sheet isn't enough alone:
/// of a recording's uploads only some time their words or gloss their lines, and which one the
/// signature returns depends on how the length rounded.
///
/// Gated on `is_ascii` as the cheap, loose stand-in for `romanize::needs_romanization`, a crate
/// away: an accented Latin sheet pays for a request it rarely needs, the right direction to fail.
fn settles_it(row: &ApiLyrics) -> bool {
    if !row.is_timed() {
        return false;
    }
    row.has_word_sync || row.is_glossed() || row.synced_lyrics.as_deref().is_some_and(str::is_ascii)
}

/// A timed sheet beats none and an untimed one. Against one already timed the bar is word times
/// or a gloss, or a length the signature matched would be traded for one merely inside the window.
fn worth_taking(exact: Option<&ApiLyrics>, found: &ApiLyrics) -> bool {
    match exact {
        Some(row) if row.is_timed() => found.has_word_sync || found.is_glossed(),
        _ => true,
    }
}

/// The row whose title, artist and length are this recording's.
///
/// **The album is deliberately not sent.** The endpoint treats it as a filter, so a compilation or
/// reissue naming the release differently is a `404` though everything else agrees. It still
/// ranks a searched row through [`album_rank`].
async fn get_exact(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    artist: &str,
    duration_ms: i64,
    ours: &Recording,
) -> Result<Option<ApiLyrics>, LookupError> {
    let mut url = endpoint(GET_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("track_name", title)
        .append_pair("artist_name", artist)
        // Rounded: a track reported a second short would spend half the window before asking.
        .append_pair("duration", &((duration_ms + 500) / 1000).to_string());

    let Some(body) = send(client, pacer, url, "Lyrics sheet", MAX_BYTES).await? else {
        return Ok(None);
    };
    let row: ApiLyrics = serde_json::from_slice(&body)
        .map_err(|e| failed("Failed to parse the lyrics response", e))?;

    Ok(answers_for(ours, &row, duration_ms).then_some(row))
}

/// Whether the signature's row is the recording asked about, on [`pick_timed`]'s terms: without
/// the album the service picks among every release, and measuring the length here keeps
/// [`DURATION_TOLERANCE_MS`] meaning one thing on both legs. A side with nothing comparable falls
/// through to the length alone.
fn answers_for(ours: &Recording, row: &ApiLyrics, duration_ms: i64) -> bool {
    row.distance_ms(duration_ms) <= DURATION_TOLERANCE_MS
        && (!ours.can_match() || ours.matches(&row.recording()))
}

/// The index's best timed row for this recording: **asked loosely, answered strictly**. The query
/// drops the `feat.` credit, the guests after the first and the album, a row filed under any of
/// them still being this song; every row back is then checked.
async fn search_timed(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    title: &str,
    credit: &ArtistCredit,
    album: &str,
    duration_ms: i64,
    ours: &Recording,
) -> Result<Option<ApiLyrics>, LookupError> {
    // Nothing this side can't compare is worth a request. The signature has no such bail: its
    // exact title and length stay comparable where the fold doesn't.
    if !ours.can_match() {
        return Ok(None);
    }

    let (track_query, artist_query) = search_terms(title, credit);
    let mut url = endpoint(SEARCH_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("track_name", track_query)
        .append_pair("artist_name", artist_query);

    let rows = search_rows(client, pacer, url).await?;
    Ok(pick_timed(rows, ours, album, duration_ms))
}

/// The searched row that is this recording, ranked by whole seconds off, then word times, then
/// gloss, then album, then the fractional distance; a full tie keeps the index's order. Split out
/// to be table-testable without a socket.
fn pick_timed(
    rows: Vec<ApiLyrics>,
    ours: &Recording,
    album: &str,
    duration_ms: i64,
) -> Option<ApiLyrics> {
    let album = fold(album);

    rows.into_iter()
        .filter(|row| {
            row.is_timed()
                && row.distance_ms(duration_ms) <= DURATION_TOLERANCE_MS
                && ours.matches(&row.recording())
        })
        .min_by(|a, b| {
            a.second_off(duration_ms)
                .cmp(&b.second_off(duration_ms))
                .then_with(|| a.kind_rank().cmp(&b.kind_rank()))
                .then_with(|| gloss_rank(a).cmp(&gloss_rank(b)))
                .then_with(|| album_rank(a, &album).cmp(&album_rank(b, &album)))
                .then_with(|| a.distance_ms(duration_ms).total_cmp(&b.distance_ms(duration_ms)))
        })
}

/// Under the second and the kind, above the album: two uploads agreeing to the second are one
/// recording twice, so what they carry decides rather than a fifth of a second of noise. A whole
/// second further out still loses.
fn gloss_rank(row: &ApiLyrics) -> u8 {
    u8::from(!row.is_glossed())
}

/// A tie-break only: the release it was ripped from is all that is left to prefer.
fn album_rank(row: &ApiLyrics, album: &str) -> u8 {
    u8::from(album.is_empty() || fold(&row.album_name) != *album)
}

/// Every row the index has for a typed query, as candidates. One request, behind the same pacer as
/// a lookup's, and none for a blank query, which the index answers with nothing.
pub(super) async fn search(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    query: &str,
    title: &str,
    credit: &ArtistCredit,
    duration_ms: i64,
) -> Result<Vec<LyricsCandidate>, LookupError> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let mut url = endpoint(SEARCH_ENDPOINT)?;
    url.query_pairs_mut().append_pair("q", query);

    let rows = search_rows(client, pacer, url).await?;
    let ours = Recording::from_credit(title, credit);
    let ranked = rank_candidates(rows, &ours, duration_ms);
    Ok(distinct(ranked.into_iter().map(ApiLyrics::into_candidate)))
}

/// The rows a person can pick among, best first: this recording's before another's, then by whole
/// seconds off, then by [`ApiLyrics::kind_rank`]; a tie keeps the index's order. A row with nothing
/// to read stays only where it calls itself instrumental.
fn rank_candidates(mut rows: Vec<ApiLyrics>, ours: &Recording, duration_ms: i64) -> Vec<ApiLyrics> {
    rows.retain(|row| row.instrumental || row.has_text());
    rows.sort_by_cached_key(|row| {
        (!ours.matches(&row.recording()), row.second_off(duration_ms), row.kind_rank())
    });
    rows
}

/// Each sheet once, at its best place. Keyed on the converted text, the Lyricsfile naming the album
/// it was filed under, so one upload filed under every release would otherwise fill the list.
fn distinct(candidates: impl Iterator<Item = LyricsCandidate>) -> Vec<LyricsCandidate> {
    let mut seen = HashSet::new();
    candidates
        .filter(|candidate| seen.insert(candidate.answer.text().map(|text| text.trim().to_owned())))
        .collect()
}

/// The index's rows for a search URL, none for a `404`.
async fn search_rows(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    url: reqwest::Url,
) -> Result<Vec<ApiLyrics>, LookupError> {
    let Some(body) = send(client, pacer, url, "Lyrics search", MAX_SEARCH_BYTES).await? else {
        return Ok(Vec::new());
    };
    serde_json::from_slice(&body)
        .map_err(|e| failed("Failed to parse the lyrics search response", e))
}

/// One GET under a cap and behind the pacer, `None` for a `404`. The single choke point, so the
/// floor, the stop and the reading of a refusal are written once.
async fn send(
    client: &reqwest::Client,
    pacer: &RequestPacer,
    url: reqwest::Url,
    what: &str,
    cap: u64,
) -> Result<Option<Vec<u8>>, LookupError> {
    if let Turn::Stopped(left) = pacer.acquire().await {
        return Err(LookupError::RateLimited { retry_after: Some(left) });
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
    // Off the status, not the body: the documented `429` payload isn't what is sent, the `503`
    // it sheds load with is undocumented, and both carry a `Retry-After`.
    if matches!(
        status,
        reqwest::StatusCode::TOO_MANY_REQUESTS | reqwest::StatusCode::SERVICE_UNAVAILABLE
    ) {
        let retry_after = retry_after(response.headers());
        pacer.stop_for(retry_after.unwrap_or(DEFAULT_BACKOFF));
        return Err(LookupError::RateLimited { retry_after });
    }
    if !status.is_success() {
        // Cloudflare's, and what a blocked `User-Agent` gets back. Named because nothing else in
        // the tree would explain it and no retry reaches it.
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

/// Delta-seconds only: the HTTP-date form would buy a date crate for a spelling rate limiters
/// don't send, and an unreadable value takes the caller's default like an absent one.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let secs: u64 = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())?;

    Some(Duration::from_secs(secs).min(MAX_BACKOFF))
}

/// Both endpoint constants parse, so the error arm is unreachable and spelled once between them.
fn endpoint(url: &'static str) -> Result<reqwest::Url, LookupError> {
    reqwest::Url::parse(url).map_err(|e| failed("Lyrics directory endpoint is not a URL", e))
}

fn failed(
    msg: &'static str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> LookupError {
    LookupError::Failed(AppError::network(msg, source))
}

#[cfg(test)]
#[path = "tests/lrclib_tests.rs"]
mod tests;
