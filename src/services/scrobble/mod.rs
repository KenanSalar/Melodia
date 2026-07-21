//! Scrobbling service: Last.fm + `ListenBrainz` "now playing" + scrobble
//! submission, fully decoupled from the player state machine.
//!
//! Phase 0 is the library-layer scaffolding only — the credential/enabled
//! shadow and the durable offline queue, loaded at boot and persisted on
//! change. The detector/submitter tasks, provider network calls, and UI wire in
//! later phases; the `Notify` submitter-wake and the shared `reqwest::Client`
//! join the struct with the Phase 2 submitter.

pub mod credentials;
pub mod model;
pub mod providers;
pub mod queue;

pub use credentials::{LastfmCredentials, ListenBrainzCredentials, ScrobbleCredentials};
pub use model::{ScrobbleTrack, scrobble_threshold_ms};
pub use queue::{QueuedItem, ScrobbleQueue};

use std::path::PathBuf;

use parking_lot::{Mutex, RwLock};

use crate::config::Paths;
use crate::error::AppResult;
use crate::services::settings::ScrobbleFlags;

/// In-memory shadow of the persisted credentials + enabled flags. Guarded by an
/// `RwLock` so the (later) detector, submitter, and love-sync can read
/// connection/enabled state synchronously without touching disk. All the
/// service's methods are synchronous, so the lock is never held across `.await`.
struct ScrobbleRuntime {
    credentials: ScrobbleCredentials,
    flags: ScrobbleFlags,
}

/// Connection + enable state for one provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderStatus {
    pub connected: bool,
    pub username: Option<String>,
    pub enabled: bool,
}

/// A cheap snapshot of scrobble state for seeding the settings UI.
#[derive(Debug, Clone, Default)]
pub struct ScrobbleStatus {
    pub lastfm: ProviderStatus,
    pub listenbrainz: ProviderStatus,
    pub love_sync_enabled: bool,
}

/// Owns the scrobble credential/enabled shadow and the durable offline queue.
/// Held as `Arc<ScrobbleService>` on [`crate::state::AppState`].
pub struct ScrobbleService {
    runtime: RwLock<ScrobbleRuntime>,
    queue: Mutex<ScrobbleQueue>,
    creds_path: PathBuf,
    queue_path: PathBuf,
}

impl ScrobbleService {
    /// Load credentials + queue from disk and seed the shadow with the enabled
    /// flags already read from `settings.json`. Infallible like
    /// [`crate::services::search_history::SearchHistoryState::init`]: a missing
    /// or corrupt file falls back to empty (logged) so boot never fails here.
    pub fn init(paths: &Paths, flags: &ScrobbleFlags) -> Self {
        let credentials = credentials::load(&paths.scrobble_credentials_path).unwrap_or_default();
        let queue = ScrobbleQueue::load(&paths.scrobble_queue_path).unwrap_or_default();
        Self {
            runtime: RwLock::new(ScrobbleRuntime {
                credentials,
                flags: flags.clone(),
            }),
            queue: Mutex::new(queue),
            creds_path: paths.scrobble_credentials_path.clone(),
            queue_path: paths.scrobble_queue_path.clone(),
        }
    }

    /// A cheap snapshot of connection + enabled state for the settings UI.
    pub fn status(&self) -> ScrobbleStatus {
        let runtime = self.runtime.read();
        ScrobbleStatus {
            lastfm: ProviderStatus {
                connected: runtime.credentials.lastfm.is_some(),
                username: runtime
                    .credentials
                    .lastfm
                    .as_ref()
                    .map(|c| c.username.clone()),
                enabled: runtime.flags.lastfm_enabled,
            },
            listenbrainz: ProviderStatus {
                connected: runtime.credentials.listenbrainz.is_some(),
                username: runtime
                    .credentials
                    .listenbrainz
                    .as_ref()
                    .map(|c| c.username.clone()),
                enabled: runtime.flags.listenbrainz_enabled,
            },
            love_sync_enabled: runtime.flags.love_sync_enabled,
        }
    }

    /// Mirror the enabled flags into the shadow after a setter has persisted
    /// them to `settings.json`, keeping synchronous readers current.
    pub fn set_flags(&self, flags: ScrobbleFlags) {
        self.runtime.write().flags = flags;
    }

    /// Connect / disconnect Last.fm: update the shadow, then persist the
    /// credential file. Passing `None` disconnects.
    pub fn set_lastfm_credentials(&self, credentials: Option<LastfmCredentials>) -> AppResult<()> {
        let snapshot = {
            let mut runtime = self.runtime.write();
            runtime.credentials.lastfm = credentials;
            runtime.credentials.clone()
        };
        credentials::save(&self.creds_path, &snapshot)
    }

    /// Connect / disconnect `ListenBrainz`: update the shadow, then persist the
    /// credential file. Passing `None` disconnects.
    pub fn set_listenbrainz_credentials(
        &self,
        credentials: Option<ListenBrainzCredentials>,
    ) -> AppResult<()> {
        let snapshot = {
            let mut runtime = self.runtime.write();
            runtime.credentials.listenbrainz = credentials;
            runtime.credentials.clone()
        };
        credentials::save(&self.creds_path, &snapshot)
    }

    /// Number of listens still queued for submission.
    pub fn queued_len(&self) -> usize {
        self.queue.lock().items.len()
    }

    /// Append a scrobble to the durable queue and persist it. The Phase 2
    /// submitter's `enqueue_scrobble` enriches and wakes on top of this
    /// primitive.
    pub fn push_scrobble(&self, item: QueuedItem) -> AppResult<()> {
        let snapshot = {
            let mut queue = self.queue.lock();
            queue.push(item);
            queue.clone()
        };
        snapshot.save(&self.queue_path)
    }
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
