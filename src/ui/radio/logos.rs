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
use std::sync::atomic::{AtomicBool, Ordering};

use lru::LruCache;
use parking_lot::Mutex;
use tokio::task::JoinSet;

use crate::error::AppError;
use crate::library;
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

/// One in-flight download: the URL asked, and what came back.
type LogoAnswer = (String, Result<Option<String>, AppError>);

/// The session's answers, keyed on the URL they came from.
///
/// `None` is "asked, nothing usable came back", and it is deliberately as durable as a hit: a
/// station whose logo is under the floor or a dead link would otherwise be re-requested every
/// time it appeared in a page.
#[derive(Debug)]
pub struct LogoMemo {
    answers: Mutex<LruCache<String, Option<String>>>,
    pruned: AtomicBool,
}

impl LogoMemo {
    pub fn new() -> Self {
        Self {
            answers: Mutex::new(LruCache::new(LOGO_MEMO_CAP)),
            pruned: AtomicBool::new(false),
        }
    }

    /// Whether this call is the one that gets to sweep the misses table, claimed once per session.
    fn claim_prune(&self) -> bool {
        !self.pruned.swap(true, Ordering::Relaxed)
    }

    /// The stored path for a URL, if this session found one.
    ///
    /// Takes the lock mutably because a hit promotes, which is what keeps the logos on screen
    /// ahead of the ones scrolled past.
    pub fn path_for(&self, favicon_url: &str) -> Option<String> {
        self.answers.lock().get(favicon_url).cloned().flatten()
    }

    /// The URLs among `favicon_urls` this session has never asked about, deduplicated and in
    /// first-seen order so the visible prefix is fetched first.
    fn unanswered<'a>(&self, favicon_urls: impl Iterator<Item = &'a str>) -> Vec<String> {
        let answers = self.answers.lock();
        let mut seen = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for url in favicon_urls.filter(|url| !url.is_empty()) {
            // `peek` rather than `get`: asking whether an answer exists must not reorder the
            // cache, or a page of already-known logos would promote itself over the page the
            // user is looking at.
            if answers.peek(url).is_none() && seen.insert(url) {
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
pub async fn fetch_missing<'a>(
    state: &AppState,
    memo: &LogoMemo,
    favicon_urls: impl Iterator<Item = &'a str>,
    is_current: impl Fn() -> bool,
    on_landed: impl Fn(),
) -> bool {
    prune_misses_once(state, memo).await;

    let mut wanted = memo.unanswered(favicon_urls);
    if wanted.is_empty() {
        return false;
    }
    drop_suppressed(state, memo, &mut wanted).await;
    if wanted.is_empty() {
        return false;
    }

    // A rolling window rather than `chunks(LOGO_BATCH)`: a chunk drained to empty before the next
    // one starts costs its slowest member, and one host at the request deadline holds five other
    // stations behind it.
    let mut pending = wanted.into_iter();
    let mut in_flight = JoinSet::new();
    for url in pending.by_ref().take(LOGO_BATCH) {
        spawn_fetch(&mut in_flight, state, url);
    }

    let mut landed = false;
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
    result: Result<Option<String>, AppError>,
) -> bool {
    let path = match result {
        Ok(path) => path,
        Err(e) => {
            // Debug rather than warn: a dead favicon is the normal condition on a directory of
            // 60,000 community-maintained entries, and the card has a monogram to fall back to.
            log::debug!("radio: logo fetch failed: {}", crate::services::describe(&e));
            memo.record(url, None);
            return false;
        }
    };

    let hit = path.is_some();
    record_outcome(state, &url, hit).await;
    memo.record(url, path);
    hit
}

/// Hand one URL to the download seam and tag the answer with what was asked.
fn spawn_fetch(in_flight: &mut JoinSet<LogoAnswer>, state: &AppState, url: String) {
    let state = state.clone();
    in_flight.spawn(async move {
        let path = library::radio::fetch_logo(&state, &url).await;
        (url, path)
    });
}

/// Take the URLs still inside a backoff out of `wanted`, memoizing them so a later page in this
/// session does not ask the table about them again.
///
/// A failure here is not worth a line: the whole feature is an optimization over asking again, and
/// asking again is what a page with no answer does.
async fn drop_suppressed(state: &AppState, memo: &LogoMemo, wanted: &mut Vec<String>) {
    let Ok(suppressed) = library::radio::suppressed_logo_urls(state, wanted).await else {
        return;
    };
    let suppressed: HashSet<String> = suppressed.into_iter().collect();
    wanted.retain(|url| !suppressed.contains(url));
    for url in suppressed {
        memo.record(url, None);
    }
}

/// Sweep the misses too old to still suppress anything, on the first page of the session.
async fn prune_misses_once(state: &AppState, memo: &LogoMemo) {
    if !memo.claim_prune() {
        return;
    }
    if let Err(e) = library::radio::prune_logo_misses(state).await {
        log::debug!("radio: logo misses not pruned: {}", crate::services::describe(&e));
    }
}

/// Carry this page's answers back to the table the next session reads.
///
/// A hit clears rather than being stored: the store path is re-derived free on the next browse,
/// and what has to go is the *old* miss, or a host that recovered stays suppressed until a
/// backoff earned when it was down finally runs out.
async fn record_outcome(state: &AppState, favicon_url: &str, hit: bool) {
    let recorded = if hit {
        library::radio::clear_logo_miss(state, favicon_url).await
    } else {
        library::radio::note_logo_miss(state, favicon_url).await
    };
    if let Err(e) = recorded {
        log::debug!("radio: logo outcome not recorded: {}", crate::services::describe(&e));
    }
}
