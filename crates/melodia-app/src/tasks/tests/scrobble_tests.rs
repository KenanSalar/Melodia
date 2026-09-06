//! The two loops either side of the queue: the submitter's retry ladder and its flush, and the
//! row enrichment the detector's effects are carried out through.
//!
//! The drain itself is `melodia-integrations`' to test and is covered there, against a local
//! server its own crate can point it at. So are the detector's *decisions*, which are the pure
//! `DetectorState`'s. What is left for here is what neither of those covers: the ladder wrapped
//! around the drain, and `fetch_row`'s single-slot cache, whose key is the only thing standing
//! between a session's second scrobble and its first track's name.
//!
//! `run_detector` itself stays undriven, and the reason is an observation rather than a seam.
//! Its priming and its shutdown arm are real decisions, but the only way to see either from
//! outside is a queued listen, and queuing one means arming a provider. `update_now_playing`
//! reads the same two flags `enqueue_scrobble` does, so an armed service turns the primed
//! view-model into an outbound POST at `listenbrainz_base`, and `with_listenbrainz_base` is
//! `pub(crate)` to `melodia-integrations`. Splitting effect production from effect execution the
//! way `handlers::evaluate_playing_tick` does would unlock both; that is a production change and
//! wants its own argument.
//!
//! Nothing here reaches the network. Last.fm is absent from all of it for `helpers.rs`' reason
//! one crate over: its readiness reads `option_env!` keys baked in at compile time, so a keyed
//! build and a keyless CI one would disagree about whether a case ran while reporting the same.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::{
    BASE_BACKOFF, MAX_BACKOFF, defer, fetch_row, process_effects, run_submitter, wait_for,
};
use melodia_core::config::Paths;
use melodia_core::entities::integrations::ScrobbleFlags;
use melodia_core::entities::track::ScrobbleRow;
use melodia_integrations::services::integrations::scrobble::detector::Effect;
use melodia_integrations::services::integrations::scrobble::{
    ListenBrainzCredentials, QueuedItem, ScrobbleService, ScrobbleTrack,
};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::database::queries::fixtures::insert_test_track;

type AnyError = Box<dyn std::error::Error>;
type TestResult = Result<(), AnyError>;

/// A service over a throwaway data root with neither provider connected, so a drain settles the
/// queue without a socket.
fn disconnected_service(dir: &std::path::Path) -> Arc<ScrobbleService> {
    let paths = Paths::rooted_at(dir.to_path_buf());
    let _ = paths.create_dirs();
    Arc::new(ScrobbleService::init(&paths, &ScrobbleFlags::default(), Arc::new(OnceLock::new())))
}

/// A service `ListenBrainz` would take a listen from, so `enqueue_scrobble` sets a flag and the
/// item reaches the queue. Connecting it also arms `update_now_playing`, which posts rather than
/// queues, so no case built on this may feed a [`Effect::NowPlaying`].
async fn armed_service(dir: &std::path::Path) -> Result<Arc<ScrobbleService>, AnyError> {
    let paths = Paths::rooted_at(dir.to_path_buf());
    let _ = paths.create_dirs();
    let flags = ScrobbleFlags {
        listenbrainz_enabled: true,
        ..ScrobbleFlags::default()
    };
    let service = Arc::new(ScrobbleService::init(&paths, &flags, Arc::new(OnceLock::new())));
    service
        .set_listenbrainz_credentials(Some(ListenBrainzCredentials {
            token: "token".to_owned(),
            username: "listener".to_owned(),
        }))
        .await?;
    Ok(service)
}

/// One row per artist, in id order. An empty artist lands as a `NULL` column, which is the shape
/// a file with no tags scans to and the one neither provider will accept.
///
/// The folder goes in first because `insert_test_track` hard-codes `folder_id: 1`.
async fn seeded_tracks(db: &DbPool, dir: &TempDir, artists: &[&str]) -> Result<Vec<i64>, AnyError> {
    queries::folder::insert_folder(db, &dir.path().to_string_lossy(), true).await?;

    let mut ids = Vec::with_capacity(artists.len());
    for (n, artist) in artists.iter().enumerate() {
        let path = dir.path().join(format!("track{n}.flac"));
        let title = format!("Track {n}");
        ids.push(
            insert_test_track(db, &path.to_string_lossy(), &title, artist, "Album", "Rock").await?,
        );
    }
    Ok(ids)
}

/// A row that says where it came from, so a case can tell one handed back by the cache from one
/// the database answered with.
fn cached_row(id: i64, title: &str) -> ScrobbleRow {
    ScrobbleRow {
        id,
        title: title.to_owned(),
        artist: Some("Artist".to_owned()),
        album: None,
        album_artist: None,
        duration_ms: 180_000,
        track_number: None,
        musicbrainz_track_id: None,
        musicbrainz_release_id: None,
    }
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

/// The cache exists so a play's now-playing read is reused at its scrobble, so its whole value is
/// being consulted at all. Handing it a row the database does not hold is what makes that visible.
#[tokio::test]
async fn a_cached_row_is_reused_rather_than_read_again() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = DbPool::test_pool().await?;
    let ids = seeded_tracks(&db, &dir, &["Artist"]).await?;
    let mut cached = Some((ids[0], cached_row(ids[0], "held in hand")));

    let found = fetch_row(&db, ids[0], &mut cached).await;

    assert_eq!(found.map(|row| row.title), Some("held in hand".to_owned()));
    Ok(())
}

/// The id is the whole of the cache's key. Without it every scrobble after the first in a session
/// carries the first track's artist and title, to both providers, and the only place it shows up
/// is the listening history the user cannot check against anything.
#[tokio::test]
async fn a_different_track_replaces_what_the_cache_was_holding() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = DbPool::test_pool().await?;
    let ids = seeded_tracks(&db, &dir, &["First", "Second"]).await?;
    let mut cached = Some((ids[0], cached_row(ids[0], "the track before")));

    let found = fetch_row(&db, ids[1], &mut cached).await;

    assert_eq!(found.map(|row| row.id), Some(ids[1]), "the row asked for, not the one in hand");
    assert_eq!(cached.map(|(id, _)| id), Some(ids[1]), "and the cache moved to it");
    Ok(())
}

/// A track deleted between its play and its scrobble is a miss, not a reason to throw away the row
/// the scrobble still to come is about to ask for.
#[tokio::test]
async fn a_missing_row_answers_nothing_and_leaves_the_cache_alone() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = DbPool::test_pool().await?;
    let ids = seeded_tracks(&db, &dir, &["Artist"]).await?;
    let mut cached = Some((ids[0], cached_row(ids[0], "still wanted")));

    let found = fetch_row(&db, ids[0] + 9_000, &mut cached).await;

    assert!(found.is_none(), "no row, no listen");
    assert_eq!(cached.map(|(id, _)| id), Some(ids[0]));
    Ok(())
}

/// The now-playing effect queues nothing, and its load is the point: it is what fills the cache the
/// scrobble reuses. Disconnected on purpose, an armed provider turning this into a POST.
#[tokio::test]
async fn a_now_playing_effect_loads_the_row_the_scrobble_will_reuse() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = DbPool::test_pool().await?;
    let ids = seeded_tracks(&db, &dir, &["Artist"]).await?;
    let service = disconnected_service(dir.path());
    let mut cached = None;

    process_effects(vec![Effect::NowPlaying { track_id: ids[0] }], &service, &db, &mut cached)
        .await;

    assert_eq!(cached.map(|(id, _)| id), Some(ids[0]));
    Ok(())
}

/// A track that played out is a listen.
#[tokio::test]
async fn a_scrobble_effect_queues_the_listen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = DbPool::test_pool().await?;
    let ids = seeded_tracks(&db, &dir, &["Artist"]).await?;
    let service = armed_service(dir.path()).await?;
    let mut cached = None;

    let effect = Effect::Scrobble {
        track_id: ids[0],
        timestamp: 1_700_000_000,
    };
    process_effects(vec![effect], &service, &db, &mut cached).await;

    assert_eq!(service.queued_len(), 1);
    Ok(())
}

/// And so is one cut short by a quit. `Finalize` shares `Scrobble`'s arm; giving it its own is a
/// one-line edit, and it silently drops every listen a shutdown finalizes, which is the only kind
/// the user has no second chance at.
#[tokio::test]
async fn a_finalize_effect_queues_a_listen_too() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = DbPool::test_pool().await?;
    let ids = seeded_tracks(&db, &dir, &["Artist"]).await?;
    let service = armed_service(dir.path()).await?;
    let mut cached = None;

    let effect = Effect::Finalize {
        track_id: ids[0],
        timestamp: 1_700_000_000,
    };
    process_effects(vec![effect], &service, &db, &mut cached).await;

    assert_eq!(service.queued_len(), 1);
    Ok(())
}

/// A row neither service will take is dropped at the enrichment rather than queued: both require a
/// non-empty artist, and a file with no tags scans to exactly that. Queued it would sit in the
/// durable file being retried and refused for the life of the install.
#[tokio::test]
async fn a_row_no_provider_can_scrobble_queues_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = DbPool::test_pool().await?;
    let ids = seeded_tracks(&db, &dir, &[""]).await?;
    let service = armed_service(dir.path()).await?;
    let mut cached = None;

    let effect = Effect::Scrobble {
        track_id: ids[0],
        timestamp: 1_700_000_000,
    };
    process_effects(vec![effect], &service, &db, &mut cached).await;

    assert_eq!(service.queued_len(), 0);
    Ok(())
}
