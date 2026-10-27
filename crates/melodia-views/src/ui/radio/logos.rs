//! Which station logos this session has, and what it costs to find out.
//!
//! A browsed station carries only a `favicon_url`, so its logo has to be fetched. The answer is
//! memoized **on the URL**, so a moved logo lands on a new file, and the fetch is a bounded burst
//! over a landed page rather than one request per mounted card, each logo reported as it lands.
//!
//! The download is `library::radio::fetch_logo`'s, behind Radio's switch; nothing here holds a
//! client or logs a URL.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use lru::LruCache;
use parking_lot::Mutex;
use tokio::task::JoinSet;

use melodia_app::library;
use melodia_app::library::radio::{LogoAnswer, SiteOrigin};
use melodia_app::state::AppState;
use melodia_core::entities::radio::StoredLogo;
use melodia_core::error::AppError;

/// How many logo answers to remember: a long session of tuning around, not the directory. An
/// eviction costs one re-request the next time that station is on screen.
const LOGO_MEMO_CAP: NonZeroUsize = match NonZeroUsize::new(2_048) {
    Some(cap) => cap,
    None => panic!("LOGO_MEMO_CAP > 0"),
};

/// Logos in flight at once. Each goes to its own host, so there is no shared quota to pace; this
/// bounds the local cost of a page landing, sockets and the decode tasks behind them.
const LOGO_BATCH: usize = 6;

/// How many stations a result may hold and still count as the one the user was looking for.
///
/// The backoff is for what a page brings along: fifty stations nobody named are worth sparing a
/// dead host for a day, but a result this narrow is what the user is looking at, and this bounds
/// the extra requests. A count rather than a "typed a name" flag, a narrow country or genre filter
/// being the same case.
const EXPLICIT_RESULT_MAX: usize = 5;

/// How hard a pass tries for the logos a page is missing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effort {
    /// A page nobody named: ask once per URL per session, and skip whatever an earlier session
    /// already found nothing at.
    Page,
    /// A result narrow enough to be the station the user just typed: ask past this session's own
    /// misses and past the stored backoff, and read the sites of whatever carries no favicon.
    Explicit,
}

impl Effort {
    /// Measured on the result, not on how much of it is unanswered, and only for a fresh query: an
    /// append or a re-warm is the same stations a second time.
    pub fn for_result(fresh: bool, results: usize) -> Self {
        if fresh && results <= EXPLICIT_RESULT_MAX { Self::Explicit } else { Self::Page }
    }
}

/// One download's URL and outcome; [`LogoAnswer`] is what the store already knew instead.
type FetchOutcome = (String, Result<Option<StoredLogo>, AppError>);

/// The session's answers, keyed on the URL. `None` is "asked, nothing usable", as durable as a hit
/// so a dead link isn't re-requested every time it appears in a page.
#[derive(Debug)]
pub struct LogoMemo {
    answers: Mutex<LruCache<String, Option<String>>>,
}

impl LogoMemo {
    pub fn new() -> Self {
        Self { answers: Mutex::new(LruCache::new(LOGO_MEMO_CAP)) }
    }

    /// The stored path for a URL, if this session found one. A hit promotes, keeping the logos on
    /// screen ahead of those scrolled past.
    pub fn path_for(&self, favicon_url: &str) -> Option<String> {
        self.answers.lock().get(favicon_url).cloned().flatten()
    }

    /// The URLs among `favicon_urls` worth asking about, deduplicated in first-seen order so the
    /// visible prefix goes first. Under [`Effort::Explicit`] a remembered `None` is no answer: a
    /// transport failure or a carried-in backoff writes one too, and a wide keystroke on the way to
    /// a narrow one would otherwise poison every URL the two share.
    fn unanswered<'a>(
        &self,
        favicon_urls: impl Iterator<Item = &'a str>,
        effort: Effort,
    ) -> Vec<String> {
        let answers = self.answers.lock();
        let mut seen = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for url in favicon_urls.filter(|url| !url.is_empty()) {
            // `peek`: asking must not promote a page of known logos over the one on screen.
            let answered = match effort {
                Effort::Page => answers.peek(url).is_some(),
                Effort::Explicit => answers.peek(url).is_some_and(Option::is_some),
            };
            if !answered && seen.insert(url) {
                out.push(url.to_owned());
            }
        }
        out
    }

    fn record(&self, favicon_url: String, path: Option<String>) {
        self.answers.lock().put(favicon_url, path);
    }
}

/// Fetches every logo a page needs that this session hasn't answered. `is_current` aborts the
/// burst once the page is superseded and `on_landed` fires per logo; returns whether any landed.
/// [`Effort::Explicit`] skips the stored backoff and this session's misses.
pub async fn fetch_missing<'a>(
    state: &AppState,
    memo: &LogoMemo,
    favicon_urls: impl Iterator<Item = &'a str>,
    effort: Effort,
    is_current: impl Fn() -> bool,
    on_landed: impl Fn(),
) -> bool {
    let mut wanted = memo.unanswered(favicon_urls, effort);
    if wanted.is_empty() {
        return false;
    }
    let mut landed = seed_from_store(state, memo, &mut wanted, effort, &on_landed).await;
    if wanted.is_empty() {
        return landed;
    }

    // A rolling window, not `chunks`: a chunk costs its slowest member.
    let mut pending = wanted.into_iter();
    let mut in_flight = JoinSet::new();
    for url in pending.by_ref().take(LOGO_BATCH) {
        spawn_fetch(&mut in_flight, state, url);
    }

    while let Some(joined) = in_flight.join_next().await {
        if !is_current() {
            in_flight.abort_all();
            break;
        }
        // Refilled first, so the window never dips while a result is recorded.
        if let Some(url) = pending.next() {
            spawn_fetch(&mut in_flight, state, url);
        }
        let Ok((url, result)) = joined else {
            // A panicked task says nothing about the URL, so nothing is memoized.
            continue;
        };
        if record_answer(state, memo, url, result).await {
            landed = true;
            on_landed();
        }
    }
    landed
}

/// Memoizes one answer, persists it where worth it, and reports whether it was a hit. `Ok(None)`
/// earns a backoff and an `Err` doesn't: a moment offline must not suppress a page of good logos
/// for a day. Both are memoized for the session.
async fn record_answer(
    state: &AppState,
    memo: &LogoMemo,
    url: String,
    result: Result<Option<StoredLogo>, AppError>,
) -> bool {
    let logo = match result {
        Ok(logo) => logo,
        Err(e) => {
            // Debug: dead favicons are routine in a community directory, and a card has a monogram.
            log::debug!("radio: logo fetch failed: {}", melodia_core::error::describe(&e));
            memo.record(url, None);
            return false;
        }
    };

    library::radio::record_logo_outcome(&state.db, &url, logo.as_ref()).await;
    let hit = logo.is_some();
    memo.record(url, logo.map(|logo| logo.path));
    hit
}

/// Reads the sites of stations with no usable favicon and memoizes what each advertises, keyed on
/// the origin the stored backoff is written under. Only under [`Effort::Explicit`], so the input
/// is bounded by [`EXPLICIT_RESULT_MAX`] and one flight covers it.
pub async fn discover_missing<'a>(
    state: &AppState,
    memo: &LogoMemo,
    sites: impl Iterator<Item = (&'a str, &'a str)>,
    is_current: impl Fn() -> bool,
    on_landed: impl Fn(),
) -> bool {
    let wanted = unasked_sites(memo, sites);
    if wanted.is_empty() {
        return false;
    }

    let mut in_flight = JoinSet::new();
    for origin in wanted {
        let state = state.clone();
        in_flight.spawn(async move {
            // Unseeded: a set this small makes the per-origin queries a handful.
            let seed = library::radio::AnswerSeed::unseeded();
            let path = library::radio::discover_site_logo(&state, &seed, &origin).await;
            (origin.as_str().to_owned(), path)
        });
    }

    let mut landed = false;
    while let Some(joined) = in_flight.join_next().await {
        if !is_current() {
            in_flight.abort_all();
            break;
        }
        let Ok((origin, path)) = joined else {
            continue;
        };
        // The facade already recorded a miss against the site.
        let hit = path.is_some();
        memo.record(origin, path);
        if hit {
            landed = true;
            on_landed();
        }
    }
    landed
}

/// The sites this session hasn't answered for, deduplicated. Unlike a favicon under
/// [`Effort::Explicit`], an empty answer isn't re-read: a site costs a whole document, and no wide
/// page records origins to have poisoned it.
fn unasked_sites<'a>(
    memo: &LogoMemo,
    sites: impl Iterator<Item = (&'a str, &'a str)>,
) -> Vec<SiteOrigin> {
    let answers = memo.answers.lock();
    let mut seen = HashSet::new();
    let mut out: Vec<SiteOrigin> = Vec::new();
    for (homepage, stream_url) in sites {
        let Some(origin) = library::radio::site_origin(homepage, stream_url) else {
            continue;
        };
        if answers.peek(origin.as_str()).is_none() && seen.insert(origin.as_str().to_owned()) {
            out.push(origin);
        }
    }
    out
}

/// Hand one URL to the download seam and tag the answer with what was asked.
fn spawn_fetch(in_flight: &mut JoinSet<FetchOutcome>, state: &AppState, url: String) {
    let state = state.clone();
    in_flight.spawn(async move {
        let path = library::radio::fetch_logo(&state, &url).await;
        (url, path)
    });
}

/// Answers what an earlier session already answered for `wanted`, reporting whether a logo came
/// back. The cold-start path: a logo's file is named by its bytes' hash, so without the answer
/// table a re-browse would re-fetch every logo.
///
/// A hit whose file has been swept stays in `wanted`, as in `kept::forget_absent_artwork`. A
/// failure isn't logged, asking again being the fallback anyway.
async fn seed_from_store(
    state: &AppState,
    memo: &LogoMemo,
    wanted: &mut Vec<String>,
    effort: Effort,
    on_landed: &impl Fn(),
) -> bool {
    let Ok(answers) = library::radio::logo_answers(&state.db, wanted).await else {
        return false;
    };

    let now = melodia_core::utils::now_rfc3339();
    let mut answered: HashSet<String> = HashSet::new();
    let mut landed = false;
    for answer in answers {
        match library::radio::classify_logo_answer(&answer, &now) {
            LogoAnswer::Hit(path) => {
                memo.record(answer.favicon_url.clone(), Some(path));
                answered.insert(answer.favicon_url);
                landed = true;
            }
            // Only this page honours the backoff; a detail visit is the exception it makes.
            LogoAnswer::Suppressed if effort == Effort::Page => {
                memo.record(answer.favicon_url.clone(), None);
                answered.insert(answer.favicon_url);
            }
            LogoAnswer::Suppressed | LogoAnswer::Unknown => {}
        }
    }

    wanted.retain(|url| !answered.contains(url));
    if landed {
        on_landed();
    }
    landed
}

#[cfg(test)]
#[path = "tests/logos_tests.rs"]
mod tests;
