//! Fixtures the scrobble suites share: a throwaway data root, a service over it, and the rows
//! both the queue and the drain are driven with.
//!
//! Shared rather than per-file because `submit_tests` drives the same service `mod_tests`
//! builds, one file over, and a second copy of a fixture is free to disagree with the first.
//!
//! Last.fm is absent from all of it, and not by oversight: reaching its submit path needs
//! `lastfm::is_configured()`, which reads keys baked in at compile time, so a keyed build and a
//! keyless CI one would disagree about whether such a test ran at all. `ListenBrainz` needs no
//! app registration, so a stored token is the whole credential and its arm is hermetic either
//! way.

use std::sync::{Arc, OnceLock};

use crate::services::integrations::scrobble::{
    ListenBrainzCredentials, QueuedItem, ScrobbleService, ScrobbleTrack,
};
use melodia_core::config::Paths;
use melodia_core::entities::integrations::ScrobbleFlags;
use melodia_core::entities::track::ScrobbleRow;

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A [`Paths`] rooted in a throwaway directory, with the subdirectories [`Paths::resolve`]
/// creates already in place. Creation is best-effort: a failure surfaces as a missing-file
/// error in the test body.
///
/// Per-crate rather than shared with the testkit, which names no workspace type — that is what
/// keeps it a leaf every other crate can dev-depend on without a cycle.
pub(crate) fn paths_in(dir: &std::path::Path) -> Paths {
    let paths = Paths::rooted_at(dir.to_path_buf());
    let _ = paths.create_dirs();
    paths
}

/// A service with a fresh (never-built) shared client `OnceLock`, so a suite that does not point
/// it at a server cannot reach the network by accident.
pub(crate) fn init_service(paths: &Paths, flags: &ScrobbleFlags) -> ScrobbleService {
    ScrobbleService::init(paths, flags, Arc::new(OnceLock::new()))
}

pub(crate) fn sample_item() -> QueuedItem {
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

/// A favorite row with its own title, so the love queue's `(artist, track)` coalescing does not
/// fold a batch into one entry.
pub(crate) fn favorite_row(id: i64, title: &str, mbid: Option<&str>) -> ScrobbleRow {
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

/// The one-row case, whose MBID is what the `ListenBrainz` love path keys on.
pub(crate) fn scrobble_row(mbid: Option<&str>) -> ScrobbleRow {
    favorite_row(1, "Song", mbid)
}

/// A service with `ListenBrainz` connected and its love toggle on or off.
pub(crate) async fn lb_love_service(
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
