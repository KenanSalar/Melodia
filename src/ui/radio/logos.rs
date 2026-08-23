//! Which station logos this session has, and what it costs to find out.
//!
//! A browsed station carries a `favicon_url` and nothing else, so the path a card draws from has
//! to be fetched. Two things follow. The answer is memoized **on the URL**, which is what makes a
//! moved logo land on a new file rather than on the one the row already had; and the fetch is a
//! pass over a landed page rather than a per-card lookup, so a screenful of stations costs one
//! bounded burst instead of one request per delegate the virtualization mounts.
//!
//! **The burst reports as it goes.** A page's logos used to land together because the whole pass
//! returned one answer at the end; the caller now hears about each one, so a card fills in when
//! its own logo arrives rather than when the slowest host on the page does.
//!
//! The download itself is `library::radio::fetch_logo`'s, past the switch that turns Radio off.
//! Nothing here holds a client, and nothing here logs a URL.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use lru::LruCache;
use parking_lot::Mutex;
use tokio::task::JoinSet;

use crate::error::AppError;
use crate::library;
use crate::media::station_logo::StoredLogo;
use crate::state::AppState;

/// How many logo answers to remember.
///
/// Sized to a long session of tuning around rather than to the directory: an entry is a URL and a
/// store path, so the whole cache is small, but a session that searches its way through tens of
/// thousands of stations has no business keeping every one of them. Evicting an answer costs one
/// re-request the next time that station is on screen.
const LOGO_MEMO_CAP: NonZeroUsize = match NonZeroUsize::new(2_048) {
    Some(cap) => cap,
    None => panic!("LOGO_MEMO_CAP > 0"),
};

/// How many logos to have in flight at once.
///
/// One request per host rather than many to one, so there is no shared quota to pace against the
/// way `services::artist_images` paces Deezer. What this bounds is the local cost of a page
/// landing: sockets, and decode-pool tasks behind them.
const LOGO_BATCH: usize = 6;

/// How many stations a result may hold and still count as the one the user was looking for.
///
/// **The backoff is about what a page brings along, not about what the user asked for.** A
/// directory page is fifty stations nobody named, so a dead favicon host on it is worth
/// suppressing for a day; a result this narrow *is* the station the user typed, they are looking
/// at it, and every extra request is bounded by this. A host that was merely down when it was
/// last asked would otherwise stay blank for up to a week of deliberate searches.
///
/// A count rather than a "did they type a name" flag: a country or genre filter narrow enough to
/// return a handful is the same situation from the user's side, and the ceiling is what bounds
/// the cost either way. Outcomes are still recorded, so a browse keeps skipping what this asks.
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
    /// Measured on the **result**, not on how much of it is still unanswered — a fifty-row page
    /// with forty-six logos in hand is still a page nobody named.
    ///
    /// Only a *fresh* query qualifies. Paging on and re-warming after a section leave are the
    /// same stations a second time, and re-asking there would spend a request per re-entry for an
    /// answer this session already has.
    pub fn for_result(fresh: bool, results: usize) -> Self {
        if fresh && results <= EXPLICIT_RESULT_MAX {
            Self::Explicit
        } else {
            Self::Page
        }
    }
}

/// One in-flight download: the URL asked, and what came back.
type LogoAnswer = (String, Result<Option<StoredLogo>, AppError>);

/// The session's answers, keyed on the URL they came from.
///
/// `None` is "asked, nothing usable came back", and it is deliberately as durable as a hit: a
/// station whose logo is under the floor or a dead link would otherwise be re-requested every
/// time it appeared in a page.
#[derive(Debug)]
pub struct LogoMemo {
    answers: Mutex<LruCache<String, Option<String>>>,
}

impl LogoMemo {
    pub fn new() -> Self {
        Self {
            answers: Mutex::new(LruCache::new(LOGO_MEMO_CAP)),
        }
    }

    /// The stored path for a URL, if this session found one.
    ///
    /// Takes the lock mutably because a hit promotes, which is what keeps the logos on screen
    /// ahead of the ones scrolled past.
    pub fn path_for(&self, favicon_url: &str) -> Option<String> {
        self.answers.lock().get(favicon_url).cloned().flatten()
    }

    /// The URLs among `favicon_urls` worth asking about, deduplicated and in first-seen order so
    /// the visible prefix is fetched first.
    ///
    /// **Under [`Effort::Explicit`] a remembered `None` is not an answer.** It is written by a
    /// genuine miss, by a transport failure, and by [`drop_suppressed`] — so a wide keystroke on
    /// the way to a narrow one poisons every URL the two share, and the narrow result's whole
    /// point is that the user is looking at these stations right now.
    fn unanswered<'a>(
        &self,
        favicon_urls: impl Iterator<Item = &'a str>,
        effort: Effort,
    ) -> Vec<String> {
        let answers = self.answers.lock();
        let mut seen = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for url in favicon_urls.filter(|url| !url.is_empty()) {
            // `peek` rather than `get`: asking whether an answer exists must not reorder the
            // cache, or a page of already-known logos would promote itself over the page the
            // user is looking at.
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

/// Fetch every logo a page needs that this session has not already answered.
///
/// `is_current` is asked between results and aborts the rest of the burst when the page it was
/// started for has been superseded; `on_landed` fires once per logo that arrives, which is what
/// lets the grid fill in a card at a time. Returns whether anything landed at all, so a caller
/// can skip the final repaint a page of already-known stations owes nothing.
///
/// [`Effort::Explicit`] skips the stored backoff entirely and re-asks what this session already
/// gave up on.
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

    // A rolling window rather than `chunks(LOGO_BATCH)`: a chunk drained to empty before the next
    // one starts costs its slowest member, and one host at the request deadline holds five other
    // stations behind it.
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
        // Refilled ahead of the handling below, so the window never dips while a result is being
        // recorded against the write connection.
        if let Some(url) = pending.next() {
            spawn_fetch(&mut in_flight, state, url);
        }
        let Ok((url, result)) = joined else {
            // A panicked download task tells us nothing about the URL, so nothing is memoized and
            // the next page carrying it asks again.
            continue;
        };
        if record_answer(state, memo, url, result).await {
            landed = true;
            on_landed();
        }
    }
    landed
}

/// Memoize one answer, persist it where it is worth persisting, and report whether it was a hit.
///
/// **`Ok(None)` earns a backoff and an `Err` does not.** One is the host saying it has nothing
/// usable, the other is not reaching the host at all, and a transport failure persisted as a miss
/// suppresses a whole page of good logos for a day over a moment offline. Both are memoized for
/// the session either way, which is what stops the same page re-asking on every scroll back.
async fn record_answer(
    state: &AppState,
    memo: &LogoMemo,
    url: String,
    result: Result<Option<StoredLogo>, AppError>,
) -> bool {
    let logo = match result {
        Ok(logo) => logo,
        Err(e) => {
            // Debug rather than warn: a dead favicon is the normal condition on a directory of
            // 60,000 community-maintained entries, and the card has a monogram to fall back to.
            log::debug!("radio: logo fetch failed: {}", crate::services::describe(&e));
            memo.record(url, None);
            return false;
        }
    };

    record_outcome(state, &url, logo.as_ref()).await;
    let hit = logo.is_some();
    memo.record(url, logo.map(|logo| logo.path));
    hit
}

/// Read the sites of stations the directory gave no usable logo for, and memoize what each one
/// advertises.
///
/// **Keyed on the origin**, which is what the station had instead of a favicon URL and what the
/// stored backoff is already written under, so the two agree about what has been asked.
///
/// Only reached under [`Effort::Explicit`], so the input is bounded by [`EXPLICIT_RESULT_MAX`] and
/// one flight covers it — there is nothing here for a rolling window to pace.
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
            let path = library::radio::discover_site_logo(&state, &origin).await;
            (origin.to_string(), path)
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
        // The miss is already recorded against the site by the facade, so this only has to
        // remember the answer for the session.
        let hit = path.is_some();
        memo.record(origin, path);
        if hit {
            landed = true;
            on_landed();
        }
    }
    landed
}

/// The sites among `sites` this session has not already answered for, deduplicated.
///
/// **A site that answered with nothing is not re-read**, where a favicon under [`Effort::Explicit`]
/// is: the cost here is a whole document, and nothing but this pass and the kept-station heal ever
/// records an origin, so there is no wide page to have poisoned it.
fn unasked_sites<'a>(
    memo: &LogoMemo,
    sites: impl Iterator<Item = (&'a str, &'a str)>,
) -> Vec<reqwest::Url> {
    let answers = memo.answers.lock();
    let mut seen = HashSet::new();
    let mut out: Vec<reqwest::Url> = Vec::new();
    for (homepage, stream_url) in sites {
        let Some(origin) = library::radio::site_origin(homepage, stream_url) else {
            continue;
        };
        if answers.peek(origin.as_str()).is_none() && seen.insert(origin.to_string()) {
            out.push(origin);
        }
    }
    out
}

/// Hand one URL to the download seam and tag the answer with what was asked.
fn spawn_fetch(in_flight: &mut JoinSet<LogoAnswer>, state: &AppState, url: String) {
    let state = state.clone();
    in_flight.spawn(async move {
        let path = library::radio::fetch_logo(&state, &url).await;
        (url, path)
    });
}

/// Answer as much of `wanted` as an earlier session already answered, and report whether any logo
/// came back that way.
///
/// **This is the cold-start path, and it is the whole reason the store is a cache.** A logo's file
/// is named by a hash of its own bytes, so nothing can know a URL's path without downloading the
/// bytes first — before the answer table there was no way to reuse a stored logo, and a re-browse
/// re-fetched every one of them and rewrote the identical file.
///
/// A hit whose file is gone stays in `wanted`: the store is swept, and a path that names nothing
/// paints an empty tile where the monogram was the honest answer. The `exists` walk is the same
/// one `kept::forget_absent_artwork` makes over a landed list, for the same reason.
///
/// A failure here is not worth a line — the feature is an optimization over asking again, and
/// asking again is what a page with no answer does.
async fn seed_from_store(
    state: &AppState,
    memo: &LogoMemo,
    wanted: &mut Vec<String>,
    effort: Effort,
    on_landed: &impl Fn(),
) -> bool {
    let Ok(answers) = library::radio::logo_answers(state, wanted).await else {
        return false;
    };

    let now = crate::utils::now_rfc3339();
    let mut answered: HashSet<String> = HashSet::new();
    let mut landed = false;
    for answer in answers {
        match answer.artwork_path {
            Some(path) if library::radio::artwork_is_present(Some(&path)) => {
                memo.record(answer.favicon_url.clone(), Some(path));
                answered.insert(answer.favicon_url);
                landed = true;
            }
            // Suppressed, and this page is not the one the backoff makes an exception for.
            None if effort == Effort::Page
                && library::radio::answer_is_suppressed(&answer, &now) =>
            {
                memo.record(answer.favicon_url.clone(), None);
                answered.insert(answer.favicon_url);
            }
            _ => {}
        }
    }

    wanted.retain(|url| !answered.contains(url));
    if landed {
        on_landed();
    }
    landed
}

/// Carry this page's answers back to the table the next session reads.
///
/// A hit stores its path, which is what lets that session draw the logo without asking; it also
/// clears whatever backoff the URL had earned, or a host that recovered would stay suppressed
/// until a schedule from when it was down finally ran out.
async fn record_outcome(state: &AppState, favicon_url: &str, logo: Option<&StoredLogo>) {
    let recorded = match logo {
        Some(logo) => library::radio::note_logo_hit(state, favicon_url, logo).await,
        None => library::radio::note_logo_miss(state, favicon_url).await,
    };
    if let Err(e) = recorded {
        log::debug!("radio: logo outcome not recorded: {}", crate::services::describe(&e));
    }
}

#[cfg(test)]
#[path = "tests/logos_tests.rs"]
mod tests;
