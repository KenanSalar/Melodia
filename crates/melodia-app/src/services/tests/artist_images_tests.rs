//! What an image pass asks Deezer, what it makes of the answer, and what it tells the lists.
//!
//! Every client here goes through a proxy that records what it is asked and refuses it, so a pass
//! that reached out shows in the record rather than failing into the same count of zero as one
//! that didn't, and nothing reaches the network. A found image's download is the one step out of
//! reach: it is held to Deezer's own hosts over https, which no local server can stand in for.

use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::{Proxy, StatusCode};

use super::{
    FetchOutcome, NO_MATCH_RETRY_AFTER, PASS_IN_FLIGHT, PASS_REQUESTED_AGAIN, PassOutcome,
    Searched, fetch_and_announce, fetch_artist_images, judge, run_passes,
};
use crate::state::Signal;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult};
use melodia_core::utils::now_unix;
use melodia_net::media::fetch::deezer::DeezerAnswer;
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::database::queries::fixtures::insert_test_track;
use melodia_testkit::http::{TestResponse, TestServer};
use tempfile::TempDir;

/// Every pass-running test reads and writes one process-global flag, so they are one test's worth
/// of state seen several times and must not overlap: one clears `PASS_IN_FLIGHT` on its way out,
/// and landing that between another's `store(true)` and its own call turns the deferral under test
/// into a real pass against a closed pool. Tokio's mutex rather than `std`'s, the guard crossing
/// an `.await` either way.
static PASS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Stages "a pass is already running" and takes it back down on the way out, whatever happens in
/// between.
///
/// A hand-written store/restore pair leaves the flag set for the rest of the binary if the
/// assertion between them fires, and every later test in this file then defers instead of running
/// — passing, and measuring nothing. It clears `PASS_REQUESTED_AGAIN` too, which the deferral sets
/// and only a live pass would otherwise read back down.
struct StagedPass;

impl StagedPass {
    fn in_flight() -> Self {
        PASS_IN_FLIGHT.store(true, Ordering::Release);
        Self
    }
}

impl Drop for StagedPass {
    fn drop(&mut self) {
        PASS_IN_FLIGHT.store(false, Ordering::Release);
        PASS_REQUESTED_AGAIN.store(false, Ordering::Release);
    }
}

const DAY_SECS: i64 = 24 * 60 * 60;

/// A data root for the artist store, kept as long as the directory it sits in.
fn data_root() -> Result<(TempDir, Paths), AppError> {
    let tmp = TempDir::new()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    Ok((tmp, paths))
}

/// A client whose every request lands on a proxy that records it and answers 502.
fn refusing_client() -> Result<(TestServer, reqwest::Client), AppError> {
    let proxy = TestServer::start(|_| TestResponse::status(502))?;
    let client = reqwest::Client::builder()
        .proxy(Proxy::all(proxy.base_url()).map_err(|e| AppError::network("proxy", e))?)
        .build()
        .map_err(|e| AppError::network("client", e))?;
    Ok((proxy, client))
}

/// A library of one artist, "Artist A", with no image.
async fn one_artist() -> Result<DbPool, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;
    insert_test_track(&db, "/music/a.mp3", "Alpha Song", "Artist A", "Album", "Rock").await?;
    Ok(db)
}

/// [`one_artist`], with every artist given an image.
async fn every_image_in_place() -> Result<DbPool, AppError> {
    let db = one_artist().await?;
    sqlx::query("UPDATE artists SET image_path = '/cached/artist.jpg'").execute(db.write()).await?;
    Ok(db)
}

/// [`one_artist`], its last search having found nothing `missed_secs_ago`.
async fn missed(missed_secs_ago: i64) -> Result<DbPool, AppError> {
    let db = one_artist().await?;
    sqlx::query("UPDATE artists SET image_search_missed_at = ? WHERE name = 'Artist A'")
        .bind(now_unix() - missed_secs_ago)
        .execute(db.write())
        .await?;
    Ok(db)
}

/// The retry window in seconds, the boundary the two miss tests sit a day either side of.
fn window_secs() -> i64 {
    i64::try_from(NO_MATCH_RETRY_AFTER.as_secs()).unwrap_or(i64::MAX)
}

#[tokio::test]
async fn a_library_whose_artists_all_have_images_asks_for_nothing() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let (_tmp, paths) = data_root()?;
    let db = every_image_in_place().await?;
    let (proxy, client) = refusing_client()?;

    fetch_artist_images(&paths, &db, &client).await?;

    assert_eq!(proxy.requests().len(), 0, "a library with every image in place reached Deezer");
    Ok(())
}

/// Driven against a closed pool, so deferring and running are distinguishable: a pass that
/// actually walked the library would fail on the first query rather than answering zero.
#[tokio::test]
async fn a_request_landing_on_a_running_pass_defers_to_it() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let (_tmp, paths) = data_root()?;
    let db = DbPool::test_pool().await?;
    db.close().await;

    let _staged = StagedPass::in_flight();
    let deferred = fetch_artist_images(&paths, &db, &reqwest::Client::new()).await;

    assert_eq!(deferred.ok(), Some(0), "a deferred request must not walk the library itself");
    Ok(())
}

/// The pass reads the clock itself, so these two sit a day either side of the window rather than
/// a second: the store's own test holds the boundary to the second.
#[tokio::test]
async fn a_recent_miss_is_not_searched_again() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let (_tmp, paths) = data_root()?;
    let db = missed(window_secs() - DAY_SECS).await?;
    let (proxy, client) = refusing_client()?;

    fetch_artist_images(&paths, &db, &client).await?;

    assert_eq!(proxy.requests().len(), 0, "an artist missed inside the window was asked again");
    Ok(())
}

#[tokio::test]
async fn a_miss_older_than_the_window_is_searched_again() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let (_tmp, paths) = data_root()?;
    let db = missed(window_secs() + DAY_SECS).await?;
    let (proxy, client) = refusing_client()?;

    fetch_artist_images(&paths, &db, &client).await?;

    assert_eq!(proxy.requests().len(), 1, "an artist missed past the window was not asked again");
    Ok(())
}

/// Every list showing the library refetches on a bump, so a pass that stored nothing sends none.
#[tokio::test]
async fn a_pass_with_nothing_to_fetch_tells_the_lists_nothing() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let (_tmp, paths) = data_root()?;
    let db = every_image_in_place().await?;
    let (_proxy, client) = refusing_client()?;
    let library_changed = Signal::new();
    let lists = library_changed.subscribe();

    fetch_and_announce(&paths, &db, &client, &library_changed).await;

    assert_eq!(lists.has_changed().ok(), Some(false));
    Ok(())
}

/// The pass it deferred to tells the lists for both.
#[tokio::test]
async fn a_deferred_request_tells_the_lists_nothing() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let (_tmp, paths) = data_root()?;
    let db = DbPool::test_pool().await?;
    db.close().await;
    let library_changed = Signal::new();
    let lists = library_changed.subscribe();
    let _staged = StagedPass::in_flight();

    fetch_and_announce(&paths, &db, &reqwest::Client::new(), &library_changed).await;

    assert_eq!(lists.has_changed().ok(), Some(false));
    Ok(())
}

/// What a search settled on, in the words the table below spells it.
fn settled(searched: &Searched) -> &'static str {
    match searched {
        Searched::Found(_) => "download",
        Searched::Settled(FetchOutcome::NoMatch) => "record a miss",
        Searched::Settled(FetchOutcome::Transient) => "retry next scan",
        Searched::Settled(FetchOutcome::Refused(_)) => "stop the pass",
        Searched::Settled(FetchOutcome::Cached(_)) => "cached",
    }
}

/// A miss keeps an artist off Deezer for the whole retry window, so it is recorded only where
/// Deezer said there is nothing: a status, an error about the query or a dropped connection says
/// nothing about the artist, and a refusal about the pace stops the pass without a verdict.
#[test]
fn only_a_definitive_no_match_is_recorded() {
    let api_error = |code: i64| Ok(DeezerAnswer::ApiError { message: "refused".to_owned(), code });
    let rows = [
        ("an image", Ok(DeezerAnswer::Body(Some("https://cdn/a.jpg".to_owned()))), "download"),
        ("no match", Ok(DeezerAnswer::Body(None)), "record a miss"),
        (
            "HTTP 429",
            Ok(DeezerAnswer::HttpStatus(StatusCode::TOO_MANY_REQUESTS)),
            "retry next scan",
        ),
        (
            "HTTP 503",
            Ok(DeezerAnswer::HttpStatus(StatusCode::SERVICE_UNAVAILABLE)),
            "retry next scan",
        ),
        ("quota, code 4", api_error(4), "stop the pass"),
        ("busy, code 700", api_error(700), "stop the pass"),
        ("no data, code 800", api_error(800), "retry next scan"),
        ("a malformed query, code 600", api_error(600), "retry next scan"),
        ("a dropped connection", Err(AppError::network_msg("connection reset")), "retry next scan"),
    ];
    for (what, answer, expected) in rows {
        assert_eq!(settled(&judge("Artist A", answer)), expected, "{what}");
    }
}

// --- the passes one fetch runs ---

/// One pass's answer, and whether a scan asked for another while it ran.
type Step = (AppResult<PassOutcome>, bool);

fn stored(fetched: u32, asked_again: bool) -> Step {
    (Ok(PassOutcome { fetched, halted: false }), asked_again)
}

fn halted(fetched: u32, asked_again: bool) -> Step {
    (Ok(PassOutcome { fetched, halted: true }), asked_again)
}

fn failed() -> Step {
    (Err(AppError::io_other("the artist query failed")), false)
}

/// Runs the passes `script` spells, answering with what the fetch answered and how many ran.
async fn run_script(script: Vec<Step>) -> (Option<u32>, usize) {
    let requested_again = AtomicBool::new(false);
    let mut script = script.into_iter();
    let mut ran = 0;
    let answer = run_passes(&requested_again, || {
        ran += 1;
        let (answer, asked_again) = script.next().unwrap_or_else(failed);
        requested_again.store(asked_again, Ordering::Release);
        async move { answer }
    })
    .await;
    (answer.ok(), ran)
}

/// A failed re-run must still answer with the images the passes before it stored, or the lists
/// are never told those artists have one now.
#[tokio::test]
async fn a_fetch_answers_with_every_image_its_passes_stored() {
    let rows = [
        ("one pass", vec![stored(2, false)], (Some(2), 1)),
        ("a request mid-pass runs one more", vec![stored(1, true), stored(3, false)], (Some(4), 2)),
        ("a halted pass spends no re-run", vec![halted(1, true), stored(5, false)], (Some(1), 1)),
        ("a failed re-run keeps the count", vec![stored(2, true), failed()], (Some(2), 2)),
        ("a failed first pass is the failure", vec![failed()], (None, 1)),
    ];
    for (what, script, expected) in rows {
        assert_eq!(run_script(script).await, expected, "{what}");
    }
}
