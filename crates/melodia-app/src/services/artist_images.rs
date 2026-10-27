use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use melodia_artwork::media::image::artwork;
use melodia_core::config::Paths;
use melodia_core::error::describe;
use melodia_core::error::{AppError, AppResult};
use melodia_core::utils::now_unix;
use melodia_net::media::fetch::deezer::{self, DeezerAnswer};
use melodia_store::database::DbPool;
use melodia_store::database::queries;

use crate::state::{AppState, Signal};

/// How long a definitive "no match" stands before the artist is searched again.
///
/// On the artist's row rather than in memory, `spawn_fetch` running after every scan, a launch's
/// re-scan included. It lapses because the directory keeps adding artists.
const NO_MATCH_RETRY_AFTER: Duration = Duration::from_hours(30 * 24);

enum FetchOutcome {
    /// Stored at this path.
    Cached(String),
    /// No usable result, recorded for [`NO_MATCH_RETRY_AFTER`].
    NoMatch,
    /// Retried on a later scan: a transport failure, or a Deezer error that answers the query
    /// rather than the pace. Unrecorded, since recording a transient one would strand the artist
    /// for the whole retry window.
    Transient,
    /// Deezer refused the search, in practice a tripped quota, so the pass stops and the next scan
    /// retries.
    Refused(String),
}

/// What a search's answer leaves the pass to do for one artist.
enum Searched {
    Found(String),
    Settled(FetchOutcome),
}

/// Only a definitive "no match" becomes [`FetchOutcome::NoMatch`]: it is the one answer recorded,
/// so a misfiled arm blanks an artist for a month rather than a scan.
fn judge(name: &str, answer: AppResult<DeezerAnswer<String>>) -> Searched {
    match answer {
        Ok(DeezerAnswer::Body(Some(url))) => Searched::Found(url),
        Ok(DeezerAnswer::Body(None)) => Searched::Settled(FetchOutcome::NoMatch),
        Ok(DeezerAnswer::HttpStatus(status)) => {
            log::warn!("Deezer answered HTTP {status} searching for '{name}'");
            Searched::Settled(FetchOutcome::Transient)
        }
        Ok(DeezerAnswer::ApiError { message, code }) => {
            let reason = format!("{message} (code {code})");
            if deezer::halts_a_batch(code) {
                return Searched::Settled(FetchOutcome::Refused(reason));
            }
            log::warn!("Deezer refused the search for '{name}': {reason}");
            Searched::Settled(FetchOutcome::Transient)
        }
        Err(e) => {
            log::warn!("Deezer search failed for '{name}': {}", describe(&e));
            Searched::Settled(FetchOutcome::Transient)
        }
    }
}

/// Deezer allows 50 requests per 5 s per IP, shared with Discord presence over the same client.
/// Five a beat apart stays well under it, for one pass at a time ([`PASS_IN_FLIGHT`]).
const BATCH_SIZE: usize = 5;
const BATCH_INTERVAL: Duration = Duration::from_millis(750);

/// Coalesces overlapping passes. [`spawn_fetch`] fires after every folder scan and
/// `reconcile_watched_folders` scans them back to back, so without it each folder's pass would
/// search the same artists at once and trip the quota. Cleared by [`PassGuard`].
static PASS_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// A request that found a pass in flight, which that pass re-runs for once it ends.
///
/// Deferred rather than dropped: a pass reads its artists once, at the start, so a dropped request
/// would leave its scan's artists for the next launch. Cleared before each pass, so one arriving
/// mid-flight can't be missed. A halted pass doesn't re-run, the quota that stopped it still being
/// closed.
static PASS_REQUESTED_AGAIN: AtomicBool = AtomicBool::new(false);

struct PassGuard;
impl Drop for PassGuard {
    fn drop(&mut self) {
        PASS_IN_FLIGHT.store(false, Ordering::Release);
    }
}

struct PassOutcome {
    fetched: u32,
    halted: bool,
}

/// Fetches a Deezer image for every artist without one, answering how many it stored. A call made
/// while a pass runs defers to that pass's re-run and answers 0.
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
    run_passes(&PASS_REQUESTED_AGAIN, || run_pass(paths, db, client)).await
}

/// Runs `pass` until one halts or no request landed while it ran. A failed re-run still answers
/// what the earlier passes stored, the caller's bump for those being all that repaints the lists.
async fn run_passes<Pass, Run>(requested_again: &AtomicBool, mut pass: Pass) -> AppResult<u32>
where
    Pass: FnMut() -> Run,
    Run: Future<Output = AppResult<PassOutcome>>,
{
    let mut fetched_total: u32 = 0;
    loop {
        requested_again.store(false, Ordering::Release);
        let outcome = match pass().await {
            Ok(outcome) => outcome,
            Err(e) if fetched_total > 0 => {
                log::warn!("Artist image re-run failed: {}", describe(&e));
                return Ok(fetched_total);
            }
            Err(e) => return Err(e),
        };
        fetched_total = fetched_total.saturating_add(outcome.fetched);

        if outcome.halted || !requested_again.load(Ordering::Acquire) {
            return Ok(fetched_total);
        }
        log::debug!("artist image fetch: re-running for a scan that landed mid-pass");
    }
}

/// One walk of the artists still without an image, a batch per [`BATCH_INTERVAL`].
async fn run_pass(paths: &Paths, db: &DbPool, client: &reqwest::Client) -> AppResult<PassOutcome> {
    let now = now_unix();
    let missed_before = now.saturating_sub_unsigned(NO_MATCH_RETRY_AFTER.as_secs());
    let artists = queries::artist::get_artists_without_images(db, missed_before).await?;
    if artists.is_empty() {
        return Ok(PassOutcome { fetched: 0, halted: false });
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
                let answer = deezer::search_artist_image_url(&client, &name).await;
                let url = match judge(&name, answer) {
                    Searched::Found(url) => url,
                    Searched::Settled(outcome) => return (id, name, outcome),
                };

                match download_and_store(&client, &url, dir).await {
                    Ok(Some(path)) => (id, name, FetchOutcome::Cached(path)),
                    // A found URL with no usable image may be a CDN hiccup.
                    Ok(None) => (id, name, FetchOutcome::Transient),
                    Err(e) => {
                        log::warn!("Failed to download image for '{name}': {}", describe(&e));
                        (id, name, FetchOutcome::Transient)
                    }
                }
            });
        }

        // One refusal per batch, which shares one closed quota window.
        let mut refusal: Option<String> = None;
        let mut refused = 0usize;
        let mut missed: Vec<i64> = Vec::with_capacity(batch.len());

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
                Ok((id, _name, FetchOutcome::NoMatch)) => missed.push(id),
                Ok((_id, _name, FetchOutcome::Transient)) => {}
                Ok((_id, _name, FetchOutcome::Refused(reason))) => {
                    refused += 1;
                    refusal.get_or_insert(reason);
                }
                Err(e) => log::error!("Artist image fetch task failed: {}", describe(&e)),
            }
        }

        if let Err(e) = queries::artist::mark_image_search_missed(db, &missed, now).await {
            log::warn!("Failed to record artist image misses: {}", describe(&e));
        }

        // A refused artist counts as unreached.
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

    Ok(PassOutcome { fetched: fetched_count, halted })
}

/// Through the shared artwork store, so an artist image is bounded and deduplicated as a cover is
/// and the two directories can't drift into two size policies.
async fn download_and_store(
    client: &reqwest::Client,
    url: &str,
    dir: PathBuf,
) -> AppResult<Option<String>> {
    let bytes = deezer::download_artist_image(client, url).await?;
    tokio::task::spawn_blocking(move || artwork::store_image(&bytes, "jpg", &dir))
        .await
        .map_err(AppError::io_source)
}

pub fn spawn_fetch(state: &AppState) {
    let paths = Arc::clone(&state.paths);
    let db = state.db.clone();
    let client = state.http_client().clone();
    let library_changed = state.library_changed.clone();
    tokio::spawn(async move { fetch_and_announce(&paths, &db, &client, &library_changed).await });
}

/// Bumps `library_changed` if the pass stored an image, the pass outliving the scan whose bump
/// would otherwise have told a painted Artists grid.
async fn fetch_and_announce(
    paths: &Paths,
    db: &DbPool,
    client: &reqwest::Client,
    library_changed: &Signal,
) {
    match fetch_artist_images(paths, db, client).await {
        Ok(0) => {}
        Ok(_) => library_changed.bump(),
        Err(e) => log::warn!("Background artist image fetch failed: {}", describe(&e)),
    }
}

#[cfg(test)]
#[path = "tests/artist_images_tests.rs"]
mod tests;
