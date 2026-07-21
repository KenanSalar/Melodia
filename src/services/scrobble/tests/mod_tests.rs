use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::{
    LastfmCredentials, ListenBrainzCredentials, QueuedItem, ScrobbleService, ScrobbleTrack,
};
use crate::config::Paths;
use crate::entities::track::ScrobbleRow;
use crate::services::settings::ScrobbleFlags;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Build a service with a fresh (never-built) shared client `OnceLock` — the
/// credential/queue tests here never touch the network.
fn init_service(paths: &Paths, flags: &ScrobbleFlags) -> ScrobbleService {
    ScrobbleService::init(paths, flags, Arc::new(OnceLock::new()))
}

/// A `Paths` rooted in a throwaway dir. Only the two scrobble paths matter here,
/// but `ScrobbleService::init` takes the whole struct.
fn paths_in(dir: &Path) -> Paths {
    Paths {
        data_dir: dir.to_path_buf(),
        db_path: dir.join("melodia.db"),
        settings_path: dir.join("settings.json"),
        view_state_path: dir.join("views.json"),
        queue_path: dir.join("queue.json"),
        search_history_path: dir.join("search_history.json"),
        scrobble_credentials_path: dir.join("scrobble_credentials.json"),
        scrobble_queue_path: dir.join("scrobble_queue.json"),
        artwork_dir: dir.join("artwork"),
        artists_dir: dir.join("artists"),
    }
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

#[test]
fn lastfm_credentials_persist_across_reinit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());

    let service = init_service(&paths, &ScrobbleFlags::default());
    service.set_lastfm_credentials(Some(LastfmCredentials {
        session_key: "sk-abc".to_owned(),
        username: "listener".to_owned(),
    }))?;
    assert!(service.status().lastfm.connected);

    // A fresh service over the same paths must read the persisted credential.
    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    let status = reloaded.status();
    assert!(status.lastfm.connected);
    assert_eq!(status.lastfm.username.as_deref(), Some("listener"));
    Ok(())
}

#[test]
fn disconnect_clears_credential() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());

    let service = init_service(&paths, &ScrobbleFlags::default());
    service.set_lastfm_credentials(Some(LastfmCredentials {
        session_key: "sk-abc".to_owned(),
        username: "listener".to_owned(),
    }))?;
    service.set_lastfm_credentials(None)?;
    assert!(!service.status().lastfm.connected);

    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    assert!(!reloaded.status().lastfm.connected);
    Ok(())
}

#[test]
fn pushed_scrobble_persists_across_reinit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());

    let service = init_service(&paths, &ScrobbleFlags::default());
    service.push_scrobble(sample_item())?;
    assert_eq!(service.queued_len(), 1);

    let reloaded = init_service(&paths, &ScrobbleFlags::default());
    assert_eq!(reloaded.queued_len(), 1);
    Ok(())
}

#[test]
fn set_flags_updates_status() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());
    assert!(!service.status().love_sync_enabled);

    service.set_flags(ScrobbleFlags {
        lastfm_enabled: true,
        listenbrainz_enabled: false,
        love_sync_enabled: true,
    });
    let status = service.status();
    assert!(status.lastfm.enabled);
    assert!(status.love_sync_enabled);
    assert!(!status.listenbrainz.enabled);
    Ok(())
}

#[test]
fn status_watch_observes_credential_and_flag_changes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = init_service(&paths_in(dir.path()), &ScrobbleFlags::default());

    // The initial value seeds the channel — disconnected on a fresh service.
    let mut rx = service.subscribe_status();
    assert!(!rx.borrow_and_update().lastfm.connected);

    // A connect publishes; the receiver sees the new username.
    service.set_lastfm_credentials(Some(LastfmCredentials {
        session_key: "sk-abc".to_owned(),
        username: "listener".to_owned(),
    }))?;
    let connected = rx.borrow_and_update().clone();
    assert!(connected.lastfm.connected);
    assert_eq!(connected.lastfm.username.as_deref(), Some("listener"));

    // A background-style auto-disconnect (submitter path) publishes too.
    service.set_lastfm_credentials(None)?;
    assert!(!rx.borrow_and_update().lastfm.connected);

    // A flag flip publishes without touching credentials.
    service.set_flags(ScrobbleFlags {
        lastfm_enabled: true,
        listenbrainz_enabled: false,
        love_sync_enabled: false,
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

/// A service with `ListenBrainz` connected + enabled and `love_sync` on/off.
fn lb_love_service(
    paths: &Paths,
    love_sync: bool,
) -> Result<ScrobbleService, Box<dyn std::error::Error>> {
    let service = init_service(
        paths,
        &ScrobbleFlags {
            lastfm_enabled: false,
            listenbrainz_enabled: true,
            love_sync_enabled: love_sync,
        },
    );
    service.set_listenbrainz_credentials(Some(ListenBrainzCredentials {
        token: "tok".to_owned(),
        username: "lb-user".to_owned(),
    }))?;
    Ok(service)
}

#[test]
fn enqueue_love_queues_listenbrainz_when_mbid_present() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), true)?;
    assert!(service.love_sync_active());

    service.enqueue_love(&scrobble_row(Some("mbid-1")), true)?;
    assert_eq!(service.queued_len(), 1);

    let queue = service.queue.lock();
    let love = queue.loves.front();
    assert!(matches!(
        love,
        Some(l) if l.loved && l.listenbrainz_remaining && !l.lastfm_remaining
    ));
    Ok(())
}

#[test]
fn enqueue_love_skips_listenbrainz_without_mbid() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), true)?;

    // No MBID for LB to key on and no Last.fm keys in a test build → nothing queued.
    service.enqueue_love(&scrobble_row(None), true)?;
    assert_eq!(service.queued_len(), 0);
    Ok(())
}

#[test]
fn love_sync_inactive_when_flag_disabled() -> TestResult {
    let dir = tempfile::tempdir()?;
    let service = lb_love_service(&paths_in(dir.path()), false)?;

    assert!(!service.love_sync_active());
    service.enqueue_love(&scrobble_row(Some("mbid-1")), true)?;
    assert_eq!(service.queued_len(), 0);
    Ok(())
}
