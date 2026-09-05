//! The submitter's drain: what it batches, what it defers, and what it does with a queue entry
//! whose provider has just gone away.
//!
//! Split in two by what a case needs. The retry policy and the batch walk are pure and are
//! driven directly; everything from the readiness gate down goes through `submit_pending`, the
//! shipped door, against a local `ListenBrainz`. Nothing here hand-rolls a copy of either: the
//! writeback guard used to be retyped in `mod_tests.rs` because the drain could not be called,
//! and pointing the drain at a server is what retired it.
//!
//! Last.fm's ready path is deliberately unreachable here. `lastfm_reachable` needs
//! `is_configured()`, which reads keys baked in at compile time, so such a test would exercise
//! the POST locally and skip on a keyless CI build while claiming the same coverage. Its policy
//! is pinned through `lastfm_reaction` instead, which needs no socket.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use melodia_testkit::http::{TestResponse, TestServer};

use super::super::tests::helpers::{
    TestResult, init_service, lb_love_service, paths_in, sample_item,
};
use super::{
    Reaction, SCROBBLE_BATCH_MAX, drop_flags, lastfm_reaction, listenbrainz_reaction, merge_opt,
    merge_retry, take_batch,
};
use crate::services::integrations::scrobble::providers::lastfm::LastfmError;
use crate::services::integrations::scrobble::providers::listenbrainz::ListenBrainzError;
use crate::services::integrations::scrobble::{
    ListenBrainzCredentials, LoveItem, QueuedItem, ScrobbleService, ScrobbleTrack,
};
use melodia_core::config::Paths;
use melodia_core::entities::integrations::ScrobbleFlags;
use melodia_core::error::AppError;

/// A queued listen with each provider's "still needs submitting" flag set as asked.
fn item_flagged(lastfm: bool, listenbrainz: bool) -> QueuedItem {
    QueuedItem {
        lastfm_remaining: lastfm,
        listenbrainz_remaining: listenbrainz,
        ..sample_item()
    }
}

/// A pending love for a track that may or may not carry the recording MBID `ListenBrainz`
/// feedback keys on. Built directly rather than through `enqueue_love`, which gates on that id
/// and so cannot produce the entry the drain's no-id arm exists for.
fn love_flagged(mbid: Option<&str>, loved: bool) -> LoveItem {
    LoveItem {
        track: ScrobbleTrack {
            recording_mbid: mbid.map(str::to_owned),
            ..sample_item().track
        },
        loved,
        lastfm_remaining: false,
        listenbrainz_remaining: true,
    }
}

/// A service with `ListenBrainz` connected, its scrobble toggle on, and every call pointed at
/// `base` rather than the public API.
async fn lb_scrobble_service(
    paths: &Paths,
    base: &str,
) -> Result<ScrobbleService, Box<dyn std::error::Error>> {
    let service = init_service(
        paths,
        &ScrobbleFlags {
            listenbrainz_enabled: true,
            listenbrainz_love_enabled: true,
            ..Default::default()
        },
    )
    .with_listenbrainz_base(base.to_owned());
    service
        .set_listenbrainz_credentials(Some(ListenBrainzCredentials {
            token: "tok".to_owned(),
            username: "lb-user".to_owned(),
        }))
        .await?;
    Ok(service)
}

// ---- the batch walk and the retry policy, no service and no socket ----

/// The cap is the provider's, so a queue longer than it goes out over several rounds rather
/// than in one POST the server would reject whole.
#[test]
fn a_batch_stops_at_the_provider_cap() {
    let snapshot: Vec<QueuedItem> = (0..60).map(|_| item_flagged(true, true)).collect();
    let (batch, idx) = take_batch(&snapshot, |it| it.lastfm_remaining);

    assert_eq!(batch.len(), SCROBBLE_BATCH_MAX);
    assert_eq!(idx.len(), SCROBBLE_BATCH_MAX);
    assert_eq!(idx.first(), Some(&0));
    assert_eq!(idx.last(), Some(&49));
}

/// The indices address the snapshot, not the batch. `collect_writeback` clears by them, so
/// numbering the batch instead would clear whichever entry happened to sit at that position.
#[test]
fn a_batch_indexes_the_queue_it_came_from_not_itself() {
    let snapshot = vec![
        item_flagged(false, true),
        item_flagged(true, true),
        item_flagged(false, true),
        item_flagged(true, true),
    ];
    let (batch, idx) = take_batch(&snapshot, |it| it.lastfm_remaining);

    assert_eq!(batch.len(), 2);
    assert_eq!(idx, vec![1, 3]);
}

/// Dropping the flags of a provider that cannot be reached is deliberately uncapped where
/// taking a batch is capped: capping it would leave the overflow pending for a provider that
/// will never clear it, and `retain_pending` would hold those entries forever.
#[test]
fn dropping_a_gone_providers_flags_is_not_capped_like_a_batch() {
    let snapshot: Vec<QueuedItem> = (0..60).map(|_| item_flagged(true, false)).collect();
    let mut out = Vec::new();
    drop_flags(&snapshot, |it| it.lastfm_remaining, &mut out);

    assert_eq!(out.len(), 60, "every pending entry must be released, not the first fifty");
}

/// Honoring several providers means honoring the longest wait, so nothing retries into a
/// window another provider already asked us to sit out.
#[test]
fn a_merged_retry_keeps_the_longest_wait() {
    let short = Duration::from_secs(5);
    let long = Duration::from_secs(90);

    assert_eq!(merge_retry(None, short), short);
    assert_eq!(merge_retry(Some(long), short), long);
    assert_eq!(merge_retry(Some(short), long), long);

    assert_eq!(merge_opt(None, None), None);
    assert_eq!(merge_opt(Some(short), None), Some(short));
    assert_eq!(merge_opt(None, Some(long)), Some(long));
    assert_eq!(merge_opt(Some(short), Some(long)), Some(long));
}

/// Only a rejected session disconnects. An unclassified code keeps its queue slot on purpose:
/// one mis-read as permanent silently loses the listen, and error 10 (invalid API key) would
/// otherwise delete a session the user never asked us to drop.
#[test]
fn only_a_rejected_lastfm_session_disconnects() {
    let deferred = Reaction::Retry(Duration::ZERO);

    assert_eq!(lastfm_reaction(&LastfmError::InvalidSession), Reaction::Disconnect);
    assert_eq!(
        lastfm_reaction(&LastfmError::Transient {
            code: 11,
            message: String::new()
        }),
        deferred
    );
    assert_eq!(
        lastfm_reaction(&LastfmError::Api {
            code: 10,
            message: String::new()
        }),
        deferred
    );
    assert_eq!(
        lastfm_reaction(&LastfmError::Transport(AppError::network_msg("no route"))),
        deferred
    );
}

/// `ListenBrainz` is the provider that names its own wait, and the three ways a 429 can arrive
/// are the whole of that policy: the header honored, absent, and past what we will sit out.
#[test]
fn a_listenbrainz_rate_limit_is_honored_defaulted_and_clamped() {
    // Seconds on both sides, since what the header asks for and what we sit out is the whole
    // of the policy.
    let honored = |reset_in_secs| match listenbrainz_reaction(&ListenBrainzError::RateLimited {
        reset_in_secs,
    }) {
        Reaction::Retry(delay) => Some(delay.as_secs()),
        Reaction::Disconnect => None,
    };

    assert_eq!(honored(Some(90)), Some(90));
    assert_eq!(honored(None), Some(30), "no header means the default wait");
    assert_eq!(honored(Some(86_400)), Some(300), "a day is clamped to what we will sit out");

    assert_eq!(listenbrainz_reaction(&ListenBrainzError::InvalidToken), Reaction::Disconnect);
    assert_eq!(
        listenbrainz_reaction(&ListenBrainzError::Server {
            status: 503,
            message: String::new()
        }),
        Reaction::Retry(Duration::ZERO)
    );
}

// ---- the writeback, a service but still no socket ----

/// A partial success is where a duplicate submission would come from: clearing both providers
/// when only one answered sends the surviving one's listen twice on the next round.
#[tokio::test]
async fn a_partial_success_clears_only_the_provider_that_answered() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());
    service.push_scrobble(item_flagged(true, true)).await?;

    let changed = service.collect_writeback(|q| &mut q.items, &[0], &[], |_, _| true);
    assert!(changed.is_some(), "clearing a flag is a change worth persisting");

    let queue = service.queue.lock();
    let item = queue.items.front();
    assert!(
        matches!(item, Some(it) if !it.lastfm_remaining && it.listenbrainz_remaining),
        "the unanswered provider stays pending: {item:?}"
    );
    Ok(())
}

/// The cap can drop the oldest entries between the snapshot and the writeback, so an index can
/// outrun the queue. It must miss rather than land on whatever slid into that slot.
#[tokio::test]
async fn a_writeback_index_past_the_queue_clears_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());
    service.push_scrobble(item_flagged(true, true)).await?;

    let changed = service.collect_writeback(|q| &mut q.items, &[7], &[9], |_, _| true);
    assert!(changed.is_none(), "nothing was cleared, so nothing needs persisting");

    let queue = service.queue.lock();
    let item = queue.items.front();
    assert!(
        matches!(item, Some(it) if it.lastfm_remaining && it.listenbrainz_remaining),
        "the surviving entry keeps both flags: {item:?}"
    );
    Ok(())
}

// ---- the drain with nothing reachable ----

/// An idle drain must not report a wait, or the submitter parks on a delay nothing asked for.
#[tokio::test]
async fn an_empty_queue_asks_for_no_retry() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());

    assert_eq!(service.submit_pending().await, None);
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

/// A listen queued for a provider the user has since disconnected has nowhere to go. Its flag
/// is released rather than left set, which is what stops it pinning the entry through
/// `retain_pending` for the life of the install.
#[tokio::test]
async fn a_listen_for_a_disconnected_provider_leaves_the_queue() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());
    let service = init_service(&paths, &ScrobbleFlags::default());
    service.push_scrobble(item_flagged(true, true)).await?;

    assert_eq!(service.submit_pending().await, None);
    assert_eq!(service.queued_len(), 0);

    // The drain persisted, so a restart does not resurrect it.
    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    assert_eq!(reloaded.queued_len(), 0);
    Ok(())
}

/// Readiness is connected **and** enabled. A stored token with the scrobble toggle off must
/// not reach the network, which is the assertion `requests()` exists for.
#[tokio::test]
async fn a_connected_provider_with_its_toggle_off_is_never_posted_to() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok("{}"))?;
    let dir = tempfile::tempdir()?;
    let service = init_service(
        &paths_in(dir.path()),
        &ScrobbleFlags {
            listenbrainz_enabled: false,
            ..Default::default()
        },
    )
    .with_listenbrainz_base(server.base_url());
    service
        .set_listenbrainz_credentials(Some(ListenBrainzCredentials {
            token: "tok".to_owned(),
            username: "lb-user".to_owned(),
        }))
        .await?;
    service.push_scrobble(item_flagged(false, true)).await?;

    assert_eq!(service.submit_pending().await, None);
    assert!(server.requests().is_empty(), "a disabled provider must not be reached");
    assert_eq!(service.queued_len(), 0, "and its flag is released rather than pinned");
    Ok(())
}

// ---- the drain against a local ListenBrainz ----

/// The happy path end to end: one POST to the submit endpoint, authenticated with the stored
/// token, and the entry gone from the queue afterwards.
#[tokio::test]
async fn an_accepted_listen_goes_out_once_and_leaves_the_queue() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok("{}"))?;
    let dir = tempfile::tempdir()?;
    let service = lb_scrobble_service(&paths_in(dir.path()), &server.base_url()).await?;
    service.push_scrobble(item_flagged(false, true)).await?;

    assert_eq!(service.submit_pending().await, None);
    assert_eq!(service.queued_len(), 0);

    let sent = server.requests();
    assert_eq!(sent.len(), 1, "one batch is one POST: {sent:?}");
    assert_eq!(sent[0].path, "/1/submit-listens");
    assert_eq!(sent[0].header("authorization"), Some("Token tok"));
    assert!(sent[0].body_text().contains("\"Song\""), "the listen is in the body");
    Ok(())
}

/// A rejected token is the one failure that deletes something the user stored. The credential
/// goes from the shadow **and** from disk, and every pending flag is released rather than left
/// to retry forever against a token we just cleared.
#[tokio::test]
async fn a_rejected_token_disconnects_and_releases_every_pending_flag() -> TestResult {
    let server = TestServer::start(|_| TestResponse::status(401))?;
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());
    let service = lb_scrobble_service(&paths, &server.base_url()).await?;
    for _ in 0..3 {
        service.push_scrobble(item_flagged(false, true)).await?;
    }

    assert_eq!(service.submit_pending().await, None, "a dead token is not a wait to honor");
    assert!(!service.status().listenbrainz.connected);
    assert_eq!(service.queued_len(), 0, "three pending flags, one disconnect");

    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    assert!(!reloaded.status().listenbrainz.connected, "the disconnect reached the file");
    Ok(())
}

/// A 429 is the provider naming its own wait. The listen stays queued and the reported delay
/// is what the header asked for, not the submitter's own ladder.
#[tokio::test]
async fn a_rate_limited_listen_stays_queued_for_the_wait_it_was_given() -> TestResult {
    let server =
        TestServer::start(|_| TestResponse::status(429).header("X-RateLimit-Reset-In", "90"))?;
    let dir = tempfile::tempdir()?;
    let service = lb_scrobble_service(&paths_in(dir.path()), &server.base_url()).await?;
    service.push_scrobble(item_flagged(false, true)).await?;

    assert_eq!(service.submit_pending().await, Some(Duration::from_secs(90)));
    assert_eq!(service.queued_len(), 1, "a deferred listen is not a dropped one");
    Ok(())
}

/// The cap is enforced on the wire, not just in the walk: sixty queued listens are one POST of
/// fifty and ten still waiting, rather than one POST the provider refuses whole.
#[tokio::test]
async fn a_queue_past_the_cap_goes_out_one_batch_at_a_time() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok("{}"))?;
    let dir = tempfile::tempdir()?;
    let service = lb_scrobble_service(&paths_in(dir.path()), &server.base_url()).await?;
    for _ in 0..60 {
        service.push_scrobble(item_flagged(false, true)).await?;
    }

    assert_eq!(service.submit_pending().await, None);
    assert_eq!(service.queued_len(), 60 - SCROBBLE_BATCH_MAX);

    let sent = server.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].body_text().matches("listened_at").count(),
        SCROBBLE_BATCH_MAX,
        "the cap is what went over the wire, not what was walked"
    );
    Ok(())
}

/// `ListenBrainz` feedback keys on the recording MBID, so a love without one has nothing to
/// send. It is marked done rather than posted, which is what stops it pinning the queue.
#[tokio::test]
async fn a_love_with_no_recording_id_is_settled_without_a_request() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok("{}"))?;
    let dir = tempfile::tempdir()?;
    let service = lb_scrobble_service(&paths_in(dir.path()), &server.base_url()).await?;
    service.queue.lock().push_love(love_flagged(None, true));

    assert_eq!(service.submit_pending().await, None);
    assert!(server.requests().is_empty(), "there is no id to key feedback on");
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

/// The score is the whole of what a love says, and it is the half a reader of the user's
/// public profile sees. An unlove must not arrive as a love.
#[tokio::test]
async fn a_love_and_an_unlove_carry_the_score_the_toggle_asked_for() -> TestResult {
    for (loved, expected) in [(true, "\"score\":1"), (false, "\"score\":0")] {
        let server = TestServer::start(|_| TestResponse::ok("{}"))?;
        let dir = tempfile::tempdir()?;
        let service = lb_scrobble_service(&paths_in(dir.path()), &server.base_url()).await?;
        service.queue.lock().push_love(love_flagged(Some("mbid-1"), loved));

        assert_eq!(service.submit_pending().await, None);

        let sent = server.requests();
        assert_eq!(sent.len(), 1, "one love is one POST: {sent:?}");
        assert_eq!(sent[0].path, "/1/feedback/recording-feedback");
        assert!(
            sent[0].body_text().contains(expected),
            "loved={loved} should send {expected}, sent {}",
            sent[0].body_text()
        );
    }
    Ok(())
}

/// Regression: a favorite toggled the opposite way while its POST is in flight coalesces the
/// fresh `loved` into the same queued entry. The writeback clears by snapshot index, so
/// without the `loved`-match guard it would clear the entry and drop that newer state, leaving
/// the user's profile disagreeing with their library until the next toggle.
///
/// The reversal happens inside the handler, which is genuinely mid-request: the drain holds no
/// queue lock across the POST.
#[tokio::test]
async fn a_love_reversed_mid_request_stays_pending() -> TestResult {
    // The handler has to reach the service and the service has to know the handler's port, so
    // the cell is what breaks the cycle: filled once the server is bound, read once it is hit.
    let under_test: Arc<OnceLock<Arc<ScrobbleService>>> = Arc::new(OnceLock::new());

    let reversing = Arc::clone(&under_test);
    let server = TestServer::start(move |_| {
        if let Some(service) = reversing.get() {
            // The user un-favorites the same track while the POST is in flight; `push_love`
            // coalesces the opposite `loved` into the queued entry.
            service.queue.lock().push_love(love_flagged(Some("mbid-1"), false));
        }
        TestResponse::ok("{}")
    })?;

    let dir = tempfile::tempdir()?;
    let service = Arc::new(
        lb_love_service(&paths_in(dir.path()), true)
            .await?
            .with_listenbrainz_base(server.base_url()),
    );
    service.queue.lock().push_love(love_flagged(Some("mbid-1"), true));
    let seated = under_test.set(Arc::clone(&service));
    assert!(seated.is_ok(), "the cell is filled once, before the only drain");

    assert_eq!(service.submit_pending().await, None);
    assert_eq!(server.requests().len(), 1, "the love was submitted");

    let queue = service.queue.lock();
    let love = queue.loves.front();
    assert!(
        matches!(love, Some(l) if !l.loved && l.listenbrainz_remaining),
        "the reversed love must still be pending: {love:?}"
    );
    Ok(())
}
