//! Which station logos this session has, and what it costs to find out.
//!
//! A browsed station carries a `favicon_url` and nothing else, so the path a card draws from has
//! to be fetched. Two things follow. The answer is memoized **on the URL**, which is what makes a
//! moved logo land on a new file rather than on the one the row already had; and the fetch is a
//! pass over a landed page rather than a per-card lookup, so a screenful of stations costs one
//! bounded burst instead of one request per delegate the virtualization mounts.
//!
//! The download itself is `library::radio::fetch_logo`'s, past the switch that turns Radio off.
//! Nothing here holds a client, and nothing here logs a URL.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};

use lru::LruCache;
use parking_lot::Mutex;

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

/// The session's answers, keyed on the URL they came from.
///
/// `None` is "asked, nothing usable came back", and it is deliberately as durable as a hit: a
/// station whose logo is under the floor or a dead link would otherwise be re-requested every
/// time it appeared in a page.
///
/// **Seeded once from the misses previous sessions recorded**, so that durability survives a
/// restart — a dead favicon host costs a full timeout per page that carries it, and a directory
/// this size has plenty of them. Only misses are seeded: a *hit* is a path in the store, which
/// the station row already carries and a browse re-derives for free.
#[derive(Debug)]
pub struct LogoMemo {
    answers: Mutex<LruCache<String, Option<String>>>,
    seeded: AtomicBool,
}

impl LogoMemo {
    pub fn new() -> Self {
        Self {
            answers: Mutex::new(LruCache::new(LOGO_MEMO_CAP)),
            seeded: AtomicBool::new(false),
        }
    }

    /// Whether this call is the one that gets to seed, claimed once for the session.
    fn claim_seeding(&self) -> bool {
        !self.seeded.swap(true, Ordering::Relaxed)
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
        let mut out: Vec<String> = Vec::new();
        for url in favicon_urls.filter(|url| !url.is_empty()) {
            // `peek` rather than `get`: asking whether an answer exists must not reorder the
            // cache, or a page of already-known logos would promote itself over the page the
            // user is looking at.
            if answers.peek(url).is_none() && !out.iter().any(|seen| seen == url) {
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
/// Returns whether anything new landed, so the caller can skip the rebuild and the repaint a page
/// of already-known stations owes nothing.
pub async fn fetch_missing<'a>(
    state: &AppState,
    memo: &LogoMemo,
    favicon_urls: impl Iterator<Item = &'a str>,
) -> bool {
    seed_from_recorded_misses(state, memo).await;

    let wanted = memo.unanswered(favicon_urls);
    if wanted.is_empty() {
        return false;
    }

    let mut landed = false;
    for batch in wanted.chunks(LOGO_BATCH) {
        let mut set = tokio::task::JoinSet::new();
        for url in batch {
            let (s, url) = (state.clone(), url.clone());
            set.spawn(async move {
                let path = library::radio::fetch_logo(&s, &url).await;
                (url, path)
            });
        }

        while let Some(joined) = set.join_next().await {
            let Ok((url, result)) = joined else {
                // A panicked download task tells us nothing about the URL, so nothing is
                // memoized and the next page carrying it asks again.
                continue;
            };
            let path = match result {
                Ok(path) => path,
                Err(e) => {
                    // Debug rather than warn: a dead favicon is the normal condition on a
                    // directory of 60,000 community-maintained entries, and the card has a
                    // glyph to fall back to.
                    log::debug!("radio: logo fetch failed: {}", crate::services::describe(&e));
                    None
                }
            };
            landed |= path.is_some();
            record_outcome(state, &url, path.is_some()).await;
            memo.record(url, path);
        }
    }
    landed
}

/// Fill the memo with what previous sessions already learned, on the first page of the session.
///
/// A failure here is not worth a line: the whole feature is an optimization over asking again,
/// and asking again is what a session with no seed does.
async fn seed_from_recorded_misses(state: &AppState, memo: &LogoMemo) {
    if !memo.claim_seeding() {
        return;
    }
    let Ok(urls) = library::radio::suppressed_logo_urls(state).await else {
        return;
    };
    for url in urls {
        memo.record(url, None);
    }
}

/// Carry this page's answers back to the table the seed reads.
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
