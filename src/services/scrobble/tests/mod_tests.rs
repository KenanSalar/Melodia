use std::sync::{Arc, OnceLock};

use super::{
    LastfmCredentials, ListenBrainzCredentials, LoveItem, LoveTarget, QueuedItem, ScrobbleService,
    ScrobbleTrack,
};
use crate::config::Paths;
use crate::entities::track::ScrobbleRow;
use crate::services::settings::ScrobbleFlags;
use crate::test_support::paths_in;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Build a service with a fresh (never-built) shared client `OnceLock` — the
/// credential/queue tests here never touch the network.
fn init_service(paths: &Paths, flags: &ScrobbleFlags) -> ScrobbleService {
    ScrobbleService::init(paths, flags, Arc::new(OnceLock::new()))
}

fn sample_item() -> QueuedItem {
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

#[test]
fn fresh_service_is_disconnected_and_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());

    let status = service.status();
    assert!(!status.lastfm.connected);
    assert!(!status.listenbrainz.connected);
    assert_eq!(status.lastfm.username, None);
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

#[tokio::test]
async fn lastfm_credentials_persist_across_reinit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());

    let service = init_service(&paths, &ScrobbleFlags::default());
    service
        .set_lastfm_credentials(Some(LastfmCredentials {
            session_key: "sk-abc".to_owned(),
            username: "listener".to_owned(),
        }))
        .await?;
    assert!(service.status().lastfm.connected);

    // A fresh service over the same paths must read the persisted credential.
    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    let status = reloaded.status();
    assert!(status.lastfm.connected);
    assert_eq!(status.lastfm.username.as_deref(), Some("listener"));
    Ok(())
}

#[tokio::test]
async fn disconnect_clears_credential() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());

    let service = init_service(&paths, &ScrobbleFlags::default());
    service
        .set_lastfm_credentials(Some(LastfmCredentials {
            session_key: "sk-abc".to_owned(),
            username: "listener".to_owned(),
        }))
        .await?;
    service.set_lastfm_credentials(None).await?;
    assert!(!service.status().lastfm.connected);

    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    assert!(!reloaded.status().lastfm.connected);
    Ok(())
}

#[tokio::test]
async fn pushed_scrobble_persists_across_reinit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());

    let service = init_service(&paths, &ScrobbleFlags::default());
    service.push_scrobble(sample_item()).await?;
    assert_eq!(service.queued_len(), 1);

    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    assert_eq!(reloaded.queued_len(), 1);
    Ok(())
}

#[test]
fn set_flags_updates_status() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());
    assert!(!service.status().listenbrainz.love_enabled);

    service.set_flags(ScrobbleFlags {
        lastfm_enabled: true,
        listenbrainz_enabled: false,
        listenbrainz_love_enabled: true,
        ..Default::default()
    });
    let status = service.status();
    assert!(status.lastfm.enabled);
    assert!(status.listenbrainz.love_enabled);
    assert!(!status.lastfm.love_enabled);
    assert!(!status.listenbrainz.enabled);
    Ok(())
}

#[tokio::test]
async fn status_watch_observes_credential_and_flag_changes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());

    // The initial value seeds the channel — disconnected on a fresh service.
    let mut rx = service.subscribe_status();
    assert!(!rx.borrow_and_update().lastfm.connected);

    // A connect publishes; the receiver sees the new username.
    service
        .set_lastfm_credentials(Some(LastfmCredentials {
            session_key: "sk-abc".to_owned(),
            username: "listener".to_owned(),
        }))
        .await?;
    let connected = rx.borrow_and_update().clone();
    assert!(connected.lastfm.connected);
    assert_eq!(connected.lastfm.username.as_deref(), Some("listener"));

    // A background-style auto-disconnect (submitter path) publishes too.
    service.set_lastfm_credentials(None).await?;
    assert!(!rx.borrow_and_update().lastfm.connected);

    // A flag flip publishes without touching credentials.
    service.set_flags(ScrobbleFlags {
        lastfm_enabled: true,
        listenbrainz_enabled: false,
        ..Default::default()
    });
    assert!(rx.borrow_and_update().lastfm.enabled);
    Ok(())
}

/// A `ScrobbleRow` with an optional recording MBID — the love-sync tests key on
/// its presence for the `ListenBrainz` path. (Last.fm love can't be exercised
/// here: it's gated on compile-time API keys, absent in a test build.)
fn scrobble_row(mbid: Option<&str>) -> ScrobbleRow {
    ScrobbleRow {
        id: 1,
        title: "Song".to_owned(),
        artist: Some("Artist".to_owned()),
        album: None,
        album_artist: None,
        duration_ms: 180_000,
        track_number: None,
        musicbrainz_track_id: mbid.map(str::to_owned),
        musicbrainz_release_id: None,
    }
}

/// A service with `ListenBrainz` connected and its love toggle on/off.
async fn lb_love_service(
    paths: &Paths,
    love_sync: bool,
) -> Result<ScrobbleService, Box<dyn std::error::Error>> {
    let service = init_service(
        paths,
        &ScrobbleFlags {
            lastfm_enabled: false,
            listenbrainz_enabled: false,
            listenbrainz_love_enabled: love_sync,
            ..Default::default()
        },
    );
    service
        .set_listenbrainz_credentials(Some(ListenBrainzCredentials {
            token: "tok".to_owned(),
            username: "lb-user".to_owned(),
        }))
        .await?;
    Ok(service)
}

#[tokio::test]
async fn enqueue_love_queues_listenbrainz_when_mbid_present() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), true).await?;
    assert!(service.love_sync_active());

    service.enqueue_love(&scrobble_row(Some("mbid-1")), true).await?;
    assert_eq!(service.queued_len(), 1);

    let queue = service.queue.lock();
    let love = queue.loves.front();
    assert!(matches!(
        love,
        Some(l) if l.loved && l.listenbrainz_remaining && !l.lastfm_remaining
    ));
    Ok(())
}

#[tokio::test]
async fn enqueue_love_skips_listenbrainz_without_mbid() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), true).await?;

    // No MBID for LB to key on and no Last.fm keys in a test build → nothing queued.
    service.enqueue_love(&scrobble_row(None), true).await?;
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

#[tokio::test]
async fn love_sync_inactive_when_flag_disabled() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), false).await?;

    assert!(!service.love_sync_active());
    service.enqueue_love(&scrobble_row(Some("mbid-1")), true).await?;
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

/// A favorite row with a distinct title so the queue's `(artist, track)`
/// coalescing doesn't fold a batch into one entry.
fn favorite_row(id: i64, title: &str, mbid: Option<&str>) -> ScrobbleRow {
    ScrobbleRow {
        id,
        title: title.to_owned(),
        artist: Some("Artist".to_owned()),
        album: None,
        album_artist: None,
        duration_ms: 180_000,
        track_number: None,
        musicbrainz_track_id: mbid.map(str::to_owned),
        musicbrainz_release_id: None,
    }
}

#[tokio::test]
async fn backfill_loves_queues_listenbrainz_favorites_with_mbid() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), true).await?;
    assert!(service.love_target_armed(LoveTarget::ListenBrainz));

    let rows = [
        favorite_row(1, "Song A", Some("mbid-1")),
        favorite_row(2, "Song B", None), // no MBID → skipped for LB
        favorite_row(3, "Song C", Some("mbid-3")),
    ];
    // One batch: only the two MBID-tagged favorites are queued.
    let queued = service.backfill_loves(&rows, LoveTarget::ListenBrainz).await?;
    assert_eq!(queued, 2);
    assert_eq!(service.queued_len(), 2);

    let queue = service.queue.lock();
    assert!(queue.loves.iter().all(|l| l.loved && l.listenbrainz_remaining && !l.lastfm_remaining));
    Ok(())
}

#[tokio::test]
async fn backfill_loves_noop_when_target_not_armed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), false).await?;
    assert!(!service.love_target_armed(LoveTarget::ListenBrainz));

    let queued = service
        .backfill_loves(&[favorite_row(1, "Song A", Some("mbid-1"))], LoveTarget::ListenBrainz)
        .await?;
    assert_eq!(queued, 0);
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

#[tokio::test]
async fn backfill_loves_lastfm_unarmed_in_keyless_build() -> TestResult {
    // Last.fm love needs compile-time API keys, absent in a test build, so the
    // target is never armed and the backfill is a no-op regardless of the flag.
    let dir = tempfile::tempdir()?;
    let service = init_service(
        &paths_in(dir.path()),
        &ScrobbleFlags {
            lastfm_love_enabled: true,
            ..Default::default()
        },
    );
    assert!(!service.love_target_armed(LoveTarget::Lastfm));
    let queued = service
        .backfill_loves(&[favorite_row(1, "Song A", Some("mbid-1"))], LoveTarget::Lastfm)
        .await?;
    assert_eq!(queued, 0);
    Ok(())
}

#[tokio::test]
async fn enqueue_loves_batches_favorites_under_one_persist() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());
    let service = lb_love_service(&paths, true).await?;
    assert!(service.love_sync_active());

    let rows = [
        favorite_row(1, "Song A", Some("mbid-1")),
        favorite_row(2, "Song B", None), // no MBID → skipped for LB (keyless build has no Last.fm)
        favorite_row(3, "Song C", Some("mbid-3")),
    ];
    // One lock + one save for the whole selection; only MBID-tagged rows queue.
    service.enqueue_loves(&rows, true).await?;
    assert_eq!(service.queued_len(), 2);
    assert!(
        service
            .queue
            .lock()
            .loves
            .iter()
            .all(|l| l.loved && l.listenbrainz_remaining && !l.lastfm_remaining)
    );

    // The single persist is real — a fresh service over the same paths reads it.
    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    assert_eq!(reloaded.queued_len(), 2);
    Ok(())
}

#[tokio::test]
async fn enqueue_loves_noop_when_love_sync_inactive() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), false).await?;
    assert!(!service.love_sync_active());

    service.enqueue_loves(&[favorite_row(1, "Song A", Some("mbid-1"))], true).await?;
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

/// Regression: a favorite toggled the opposite way while a love POST is in
/// flight coalesces a fresh `loved` into the same queued entry (`push_love`).
/// The submit writeback clears by snapshot index, so without the `loved`-match
/// guard it would drop that newer state. The guard must leave the reversed love
/// pending so it goes out next round.
#[tokio::test]
async fn love_writeback_keeps_a_concurrently_reversed_toggle_pending() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), true).await?;

    // A love (heart) is queued, then snapshotted as the submitter would before
    // POSTing it.
    service.enqueue_love(&scrobble_row(Some("mbid-1")), true).await?;
    let snapshot: Vec<LoveItem> = service.queue.lock().loves.iter().cloned().collect();

    // Mid-flight, the user un-favorites the same track: the opposite `loved`
    // coalesces into the queued entry.
    let Some(track) = ScrobbleTrack::from_row(&scrobble_row(Some("mbid-1"))) else {
        return Err("scrobble row should build".into());
    };
    service.queue.lock().push_love(LoveItem {
        track,
        loved: false,
        lastfm_remaining: false,
        listenbrainz_remaining: true,
    });

    // The writeback would clear index 0's LB flag on the POST's success, but the
    // guard sees `loved` flipped (true → false) and skips it: nothing cleared,
    // nothing removed.
    let changed = service.collect_writeback(
        |q| &mut q.loves,
        &[],
        &[0],
        |i, current: &LoveItem| snapshot.get(i).is_some_and(|s| s.loved == current.loved),
    );
    assert!(changed.is_none());

    let queue = service.queue.lock();
    let Some(love) = queue.loves.front() else {
        return Err("reversed love should still be queued".into());
    };
    assert!(!love.loved);
    assert!(love.listenbrainz_remaining);
    Ok(())
}
