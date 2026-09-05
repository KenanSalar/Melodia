//! The submitter loop: how long a deferral parks it, and what a shutdown owes the queue.
//!
//! The drain itself is `melodia-integrations`' to test and is covered there, against a local
//! server its own crate can point it at. What is here is the half that crate cannot see: the
//! retry ladder wrapped around the drain, and the flush on the way out. `run_detector` is not
//! driven from here, its decisions being the pure `DetectorState`'s and covered by that
//! module's suite.
//!
//! Nothing here reaches the network. The flush is driven through a service with no credentials,
//! whose drain releases the pending flags rather than posting them, so what the loop does with
//! a queue on shutdown is observable without a provider on the other end.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{BASE_BACKOFF, MAX_BACKOFF, defer, run_submitter, wait_for};
use melodia_core::config::Paths;
use melodia_core::entities::integrations::ScrobbleFlags;
use melodia_integrations::services::integrations::scrobble::{
    QueuedItem, ScrobbleService, ScrobbleTrack,
};

type AnyError = Box<dyn std::error::Error>;
type TestResult = Result<(), AnyError>;

/// A service over a throwaway data root with neither provider connected, so a drain settles the
/// queue without a socket.
fn disconnected_service(dir: &std::path::Path) -> Arc<ScrobbleService> {
    let paths = Paths::rooted_at(dir.to_path_buf());
    let _ = paths.create_dirs();
    Arc::new(ScrobbleService::init(&paths, &ScrobbleFlags::default(), Arc::new(OnceLock::new())))
}

/// A pending listen. Pushed rather than enqueued on purpose: pushing does not wake the
/// submitter, which is what leaves it parked for the shutdown to find.
fn queued_listen() -> QueuedItem {
    QueuedItem {
        track: ScrobbleTrack {
            artist: "Artist".to_owned(),
            track: "Song".to_owned(),
            album: None,
            album_artist: None,
            duration_secs: Some(180),
            track_number: None,
            recording_mbid: None,
            release_mbid: None,
        },
        timestamp: 1_700_000_000,
        lastfm_remaining: true,
        listenbrainz_remaining: true,
    }
}

/// A provider that names its own window has measured something we can only guess at, so its
/// wait wins whenever it is the longer of the two. The ladder still advances, so a second
/// deferral is not retried at the same rate as the first.
#[test]
fn a_providers_own_wait_wins_over_the_local_ladder() {
    let asked = Duration::from_secs(90);
    let (wait, next) = defer(asked, BASE_BACKOFF);

    assert_eq!(wait, asked, "a 429 asking 90s is not retried at 15s");
    assert_eq!(next, BASE_BACKOFF * 2);

    // And the other way: a provider naming nothing falls back to the ladder.
    let (wait, _) = defer(Duration::ZERO, BASE_BACKOFF);
    assert_eq!(wait, BASE_BACKOFF);
}

/// The ladder doubles and then stops. Without the ceiling a long outage walks the wait past any
/// plausible session, and a listen queued early would never be retried at all.
#[test]
fn the_backoff_ladder_doubles_up_to_its_ceiling_and_stays_there() {
    let mut backoff = BASE_BACKOFF;
    let mut seen = Vec::new();
    for _ in 0..12 {
        let (_, next) = defer(Duration::ZERO, backoff);
        backoff = next;
        seen.push(backoff);
    }

    assert_eq!(seen.first(), Some(&(BASE_BACKOFF * 2)));
    assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "the ladder never goes backwards");
    assert!(seen.iter().all(|&d| d <= MAX_BACKOFF), "and never past its ceiling: {seen:?}");
    assert_eq!(seen.last(), Some(&MAX_BACKOFF), "twelve deferrals is well past it");
}

/// `None` is how the loop parks until something wakes it, and it must not resolve on its own.
/// A `Some` elapses on the clock rather than in real time.
#[tokio::test(start_paused = true)]
async fn an_unset_wait_parks_where_a_set_one_elapses() {
    let parked = tokio::time::timeout(MAX_BACKOFF * 2, wait_for(None)).await;
    assert!(parked.is_err(), "an unset wait is not a delay, it is a park");

    let started = tokio::time::Instant::now();
    wait_for(Some(BASE_BACKOFF)).await;
    assert!(started.elapsed() >= BASE_BACKOFF);
}

/// A listen queued while the submitter is parked must not sit there until the next launch. The
/// shutdown arm is `biased` first precisely so the flush runs before the loop notices anything
/// else, and it is the only thing standing between a quit and a queue nobody drains.
#[tokio::test]
async fn a_shutdown_flushes_what_is_still_queued() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = disconnected_service(dir.path());

    let shutdown = CancellationToken::new();
    let submitter = tokio::spawn(run_submitter(shutdown.clone(), Arc::clone(&service)));

    // Let the loop take its entry drain on the empty queue and park, so what follows is the
    // flush rather than that first pass finding the item.
    tokio::task::yield_now().await;
    service.push_scrobble(queued_listen()).await?;
    assert_eq!(service.queued_len(), 1, "test setup: parked with work waiting");

    shutdown.cancel();
    submitter.await?;

    assert_eq!(service.queued_len(), 0, "the flush drained the queue on the way out");
    Ok(())
}
