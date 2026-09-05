use super::helpers::{
    TestResult, favorite_row, init_service, lb_love_service, paths_in, sample_item, scrobble_row,
};
use crate::services::integrations::scrobble::{LastfmCredentials, LoveTarget};
use melodia_core::entities::integrations::ScrobbleFlags;

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

    // No MBID for LB to key on, and Last.fm's love toggle is off in this fixture, so no
    // provider wants the row.
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

/// Arming needs a stored session as well as the toggle, so turning love-sync on before
/// connecting queues nothing. Asserted on the session rather than on `is_configured()`, which
/// reads keys baked in at compile time and so differs between a keyed build and a keyless one.
#[tokio::test]
async fn backfill_loves_is_unarmed_for_lastfm_without_a_stored_session() -> TestResult {
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
