use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::AppResult;
use crate::error::describe;
use crate::media::deezer::{self, DeezerAnswer};

/// Session-scoped negative memo: artist ids whose Deezer search returned a
/// definitive "no match" this session. `spawn_fetch` runs after every scan
/// completion, so without this every image-less artist is re-queried over
/// the network per scan — and the answer won't change until tags change
/// (which usually creates new artist rows anyway). Transient failures
/// (transport/HTTP errors, failed downloads) are deliberately NOT memoized
/// so a later scan retries them. Cleared only by process restart; bounded
/// by the artist-table size (bare `i64`s).
fn attempted_no_match() -> &'static Mutex<HashSet<i64>> {
    static ATTEMPTED: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    ATTEMPTED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Per-artist result of one Deezer round-trip.
enum FetchOutcome {
    /// Image found, downloaded, and cached at this path.
    Cached(String),
    /// Deezer answered definitively with no usable result — memoized.
    NoMatch,
    /// Transport/HTTP/download failure — retry on a later scan. Also where a
    /// Deezer error code that isn't [`deezer::halts_a_batch`] lands: those answer
    /// the query rather than the pace, so the pass keeps going and the answer is
    /// deliberately not memoized. A name that reliably draws one therefore costs
    /// a request per scan for the session, which is the right way round —
    /// memoizing would strand a genuinely transient failure for just as long, and
    /// it is one request either way.
    Transient,
    /// Deezer refused the search itself, carrying its own reason. In practice a
    /// tripped quota, which the remaining batches would spend on refusals too —
    /// so this stops the pass. Nothing is memoized, so the next scan retries.
    Refused(String),
}

/// Deezer's ceiling is 50 requests per 5 s per IP, and the Discord presence path
/// spends from the same budget over the same client. Five at a time a beat apart
/// stays well under it while still clearing a first-launch backlog quickly — but
/// only for one pass at a time, which is what [`PASS_IN_FLIGHT`] is for.
const BATCH_SIZE: usize = 5;
const BATCH_INTERVAL: Duration = Duration::from_millis(750);

/// Coalesces overlapping passes, and the request budget above is why.
///
/// [`spawn_fetch`] is detached and fires at the end of *every* folder scan, while
/// `reconcile_watched_folders` walks the enabled folders back to back — so folder
/// one's pass is still running when folder two's starts. Each takes its own
/// `get_artists_without_images` snapshot, and an image leaves that list only once
/// its download has landed in the DB, so the snapshots overlap almost entirely:
/// N folders means N times the concurrency spent on the same artists, which is
/// what trips the quota the pacing above exists to respect.
///
/// RAII-cleared by [`PassGuard`], so an early return can't strand it. The
/// `reconcile_watched_folders` shape, one service over.
static PASS_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// **Deferred, not dropped** — the half `reconcile_watched_folders` has no need of
/// and this does.
///
/// A pass queries once, at the start, so the artists a *later* scan commits are
/// not in its snapshot. Dropping the overlapping request would therefore lose them
/// until some future scan, and on a first launch with several folders that means
/// the next launch: folder one's pass takes the snapshot and folders two onward
/// never get one. So a skipped request records itself here, and the running pass
/// re-runs once it finishes. Cleared *before* each pass rather than after, so a
/// request arriving mid-flight is the one thing the check can't miss.
///
/// A halted pass is the exception that doesn't re-run: it stopped because Deezer
/// said to stop asking, and a queued request would spend itself on the same closed
/// window — which is the reason the pass stopped.
static PASS_REQUESTED_AGAIN: AtomicBool = AtomicBool::new(false);

struct PassGuard;
impl Drop for PassGuard {
    fn drop(&mut self) {
        PASS_IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// What one pass did, for the re-run decision above.
struct PassOutcome {
    fetched: u32,
    halted: bool,
}

/// Fetch artist images from Deezer for artists that don't have one yet.
///
/// One pass at a time: a call made while another is running defers to that pass's
/// re-run rather than doubling the concurrency spent on the same artists. Uses the
/// shared `reqwest::Client` from `AppState` so the connection pool is reused
/// across every HTTP-using service.
pub async fn fetch_artist_images(
    paths: &Paths,
    db: &DbPool,
    client: &reqwest::Client,
) -> AppResult<u32> {
    if PASS_IN_FLIGHT.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        PASS_REQUESTED_AGAIN.store(true, Ordering::Release);
        log::debug!("artist image fetch: a pass is in flight, deferring to its re-run");
        return Ok(0);
    }
    let _guard = PassGuard;

    let mut fetched_total: u32 = 0;
    loop {
        PASS_REQUESTED_AGAIN.store(false, Ordering::Release);
        let outcome = run_pass(paths, db, client).await?;
        fetched_total = fetched_total.saturating_add(outcome.fetched);

        if outcome.halted || !PASS_REQUESTED_AGAIN.load(Ordering::Acquire) {
            return Ok(fetched_total);
        }
        log::debug!("artist image fetch: re-running for a scan that landed mid-pass");
    }
}

/// One walk of the artists that still have no image.
///
/// Runs one [`BATCH_SIZE`] batch of concurrent searches per [`BATCH_INTERVAL`],
/// and abandons the rest if Deezer refuses on a code that says to stop asking.
async fn run_pass(paths: &Paths, db: &DbPool, client: &reqwest::Client) -> AppResult<PassOutcome> {
    let mut artists = queries::artist::get_artists_without_images(db).await?;
    {
        let attempted = attempted_no_match().lock();
        artists.retain(|a| !attempted.contains(&a.id));
    }
    if artists.is_empty() {
        return Ok(PassOutcome {
            fetched: 0,
            halted: false,
        });
    }

    let artists_dir = paths.artists_dir.clone();
    let mut fetched_count: u32 = 0;
    let mut processed = 0usize;
    let mut halted = false;

    for (batch_idx, batch) in artists.chunks(BATCH_SIZE).enumerate() {
        if batch_idx > 0 {
            tokio::time::sleep(BATCH_INTERVAL).await;
        }
        let mut set = tokio::task::JoinSet::new();

        for artist in batch {
            let client = client.clone();
            let name = artist.name.clone();
            let id = artist.id;
            let dir = artists_dir.clone();

            set.spawn(async move {
                let url = match deezer::search_artist_image_url(&client, &name).await {
                    Ok(DeezerAnswer::Body(Some(url))) => url,
                    Ok(DeezerAnswer::Body(None)) => return (id, name, FetchOutcome::NoMatch),
                    Ok(DeezerAnswer::HttpStatus(status)) => {
                        log::warn!("Deezer answered HTTP {status} searching for '{name}'");
                        return (id, name, FetchOutcome::Transient);
                    }
                    Ok(DeezerAnswer::ApiError { message, code }) => {
                        let reason = format!("{message} (code {code})");
                        if deezer::halts_a_batch(code) {
                            return (id, name, FetchOutcome::Refused(reason));
                        }
                        log::warn!("Deezer refused the search for '{name}': {reason}");
                        return (id, name, FetchOutcome::Transient);
                    }
                    Err(e) => {
                        log::warn!("Deezer search failed for '{name}': {}", describe(&e));
                        return (id, name, FetchOutcome::Transient);
                    }
                };

                match deezer::download_and_cache_artist_image(&client, &url, &dir).await {
                    Ok(Some(path)) => (id, name, FetchOutcome::Cached(path)),
                    // A found URL that yielded no cacheable image could be a
                    // CDN hiccup — treat as transient so a later scan retries.
                    Ok(None) => (id, name, FetchOutcome::Transient),
                    Err(e) => {
                        log::warn!("Failed to download image for '{name}': {}", describe(&e));
                        (id, name, FetchOutcome::Transient)
                    }
                }
            });
        }

        // The whole batch shares one closed quota window, so it reports one
        // refusal rather than five identical warnings.
        let mut refusal: Option<String> = None;
        let mut refused = 0usize;

        while let Some(result) = set.join_next().await {
            match result {
                Ok((id, name, FetchOutcome::Cached(path))) => {
                    if let Err(e) = queries::artist::update_artist_image_path(db, id, &path).await {
                        log::warn!(
                            "Failed to update artist image path for '{name}': {}",
                            describe(&e)
                        );
                    } else {
                        fetched_count += 1;
                    }
                }
                Ok((id, _name, FetchOutcome::NoMatch)) => {
                    attempted_no_match().lock().insert(id);
                }
                Ok((_id, _name, FetchOutcome::Transient)) => {}
                Ok((_id, _name, FetchOutcome::Refused(reason))) => {
                    refused += 1;
                    refusal.get_or_insert(reason);
                }
                Err(e) => log::error!("Artist image fetch task failed: {e}"),
            }
        }

        // Refusals don't count: Deezer declined to answer for those, so they sit
        // with the artists the pass never reached rather than with the ones it
        // got a round trip out of.
        processed += batch.len() - refused;

        if let Some(reason) = refusal {
            log::warn!(
                "Deezer refused the artist-image search ({reason}); stopping this pass, {} artist(s) unanswered and retried on the next scan",
                artists.len() - processed
            );
            halted = true;
            break;
        }
    }

    // Fetched images land in the DB but nothing signals the UI, so a grid
    // that's already painted won't pick them up until its next refresh.
    Ok(PassOutcome {
        fetched: fetched_count,
        halted,
    })
}

/// Spawn a background task to fetch artist images. Fire-and-forget.
/// `client` is the shared client from `AppState::http_client()`.
pub fn spawn_fetch(paths: Arc<Paths>, db: DbPool, client: reqwest::Client) {
    tokio::spawn(async move {
        if let Err(e) = fetch_artist_images(&paths, &db, &client).await {
            log::warn!("Background artist image fetch failed: {}", describe(&e));
        }
    });
}
