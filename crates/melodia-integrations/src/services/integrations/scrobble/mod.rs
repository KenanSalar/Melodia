//! Last.fm and `ListenBrainz`: now-playing, the durable listen and love queue, and the drain that
//! submits it. A read-only tap on the player (`docs/adr/0023-scrobbling-is-a-read-only-tap.md`):
//! [`detector`] and [`live_detector`] decide, and `tasks::scrobble` drives them and calls in here.

pub mod credentials;
pub mod detector;
pub mod live_detector;
pub mod model;
pub mod providers;
pub mod queue;

mod status;
mod submit;

pub use credentials::{LastfmCredentials, ListenBrainzCredentials, ScrobbleCredentials};
pub use model::{ScrobbleTrack, scrobble_threshold_ms};
pub use queue::{LoveItem, QueuedItem, ScrobbleQueue};
pub use status::{LoveTarget, ProviderStatus, ScrobbleStatus};

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};
use reqwest::Client;
use tokio::sync::{Notify, watch};

use melodia_core::config::Paths;
use melodia_core::entities::integrations::ScrobbleFlags;
use melodia_core::entities::track::ScrobbleRow;
use melodia_core::error::{AppError, AppResult, describe};
use melodia_net::services::net::build_http_client;
use providers::lastfm;
use providers::listenbrainz;
use status::build_status;

/// The persisted credentials and flags, mirrored so the detector, submitter and love sync read
/// them without touching disk. The guard is never held across an `.await`.
struct ScrobbleRuntime {
    credentials: ScrobbleCredentials,
    flags: ScrobbleFlags,
}

impl ScrobbleRuntime {
    /// App keys shipped and a session stored. Not `connected`, which [`ProviderStatus`] spells
    /// for the session alone.
    fn lastfm_reachable(&self) -> bool {
        lastfm::is_configured() && self.credentials.lastfm.is_some()
    }

    /// The user's token is the whole credential; there are no app keys.
    fn listenbrainz_reachable(&self) -> bool {
        self.credentials.listenbrainz.is_some()
    }

    fn lastfm_scrobble_ready(&self) -> bool {
        self.lastfm_reachable() && self.flags.lastfm_enabled
    }

    fn listenbrainz_scrobble_ready(&self) -> bool {
        self.listenbrainz_reachable() && self.flags.listenbrainz_enabled
    }

    fn lastfm_love_armed(&self) -> bool {
        self.lastfm_reachable() && self.flags.lastfm_love_enabled
    }

    /// Callers queueing a love also skip a track with no `recording_mbid`, which feedback keys on.
    fn listenbrainz_love_armed(&self) -> bool {
        self.listenbrainz_reachable() && self.flags.listenbrainz_love_enabled
    }
}

/// Held as `Arc<ScrobbleService>` on the app's `AppState`.
pub struct ScrobbleService {
    runtime: RwLock<ScrobbleRuntime>,
    queue: Mutex<ScrobbleQueue>,
    creds_path: PathBuf,
    queue_path: PathBuf,
    notify: Notify,
    /// `AppState`'s lazy client, so the whole app shares one connection pool.
    http: Arc<OnceLock<Client>>,
    /// [`listenbrainz::LB_API_BASE`] everywhere but the submitter's suites.
    listenbrainz_base: String,
    status_tx: watch::Sender<ScrobbleStatus>,
}

impl ScrobbleService {
    /// Infallible, so boot never fails here: an unreadable file loads empty.
    pub fn init(paths: &Paths, flags: &ScrobbleFlags, http: Arc<OnceLock<Client>>) -> Self {
        let credentials = credentials::load(&paths.scrobble_credentials_path).unwrap_or_default();
        let queue = ScrobbleQueue::load(&paths.scrobble_queue_path).unwrap_or_default();
        let (status_tx, _) = watch::channel(build_status(&credentials, flags));
        Self {
            runtime: RwLock::new(ScrobbleRuntime { credentials, flags: flags.clone() }),
            queue: Mutex::new(queue),
            creds_path: paths.scrobble_credentials_path.clone(),
            queue_path: paths.scrobble_queue_path.clone(),
            notify: Notify::new(),
            http,
            listenbrainz_base: listenbrainz::LB_API_BASE.to_owned(),
            status_tx,
        }
    }

    /// Points the `ListenBrainz` calls at a local server. Consuming, so a seated `Arc` can't
    /// reach it.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_listenbrainz_base(mut self, base: String) -> Self {
        self.listenbrainz_base = base;
        self
    }

    pub fn status(&self) -> ScrobbleStatus {
        let runtime = self.runtime.read();
        build_status(&runtime.credentials, &runtime.flags)
    }

    /// What the settings rows follow, so an auto-disconnect shows without reopening the section.
    pub fn subscribe_status(&self) -> watch::Receiver<ScrobbleStatus> {
        self.status_tx.subscribe()
    }

    /// Mirrors flags a setter has already persisted to `settings.json`.
    pub fn set_flags(&self, flags: ScrobbleFlags) {
        self.runtime.write().flags = flags;
        self.status_tx.send_replace(self.status());
    }

    /// Publishes the change before writing it, so a failed persist still shows the state the
    /// user asked for.
    async fn persist_credentials_change(
        &self,
        update: impl FnOnce(&mut ScrobbleCredentials),
    ) -> AppResult<()> {
        let snapshot = {
            let mut runtime = self.runtime.write();
            update(&mut runtime.credentials);
            runtime.credentials.clone()
        };
        self.status_tx.send_replace(self.status());
        let path = self.creds_path.clone();
        match tokio::task::spawn_blocking(move || credentials::save(&path, &snapshot)).await {
            Ok(result) => result,
            Err(e) => Err(AppError::io("scrobble credential persist task panicked", e)),
        }
    }

    /// `None` disconnects.
    pub async fn set_lastfm_credentials(
        &self,
        credentials: Option<LastfmCredentials>,
    ) -> AppResult<()> {
        self.persist_credentials_change(move |creds| creds.lastfm = credentials).await
    }

    /// `None` disconnects.
    pub async fn set_listenbrainz_credentials(
        &self,
        credentials: Option<ListenBrainzCredentials>,
    ) -> AppResult<()> {
        self.persist_credentials_change(move |creds| creds.listenbrainz = credentials).await
    }

    /// Loves count too: the submitter re-drains while this is non-zero.
    pub fn queued_len(&self) -> usize {
        let queue = self.queue.lock();
        queue.items.len() + queue.loves.len()
    }

    /// Queues and persists without waking the submitter, which [`Self::enqueue_track`] adds.
    pub async fn push_scrobble(&self, item: QueuedItem) -> AppResult<()> {
        let snapshot = {
            let mut queue = self.queue.lock();
            queue.push(item);
            queue.clone()
        };
        self.save_queue(snapshot).await
    }

    /// Propagates the write's result; `submit::persist_queue` is the log-only form.
    async fn save_queue(&self, snapshot: ScrobbleQueue) -> AppResult<()> {
        let path = self.queue_path.clone();
        match tokio::task::spawn_blocking(move || snapshot.save(&path)).await {
            Ok(result) => result,
            Err(e) => Err(AppError::io("scrobble queue persist task panicked", e)),
        }
    }

    /// The cheap gate `library::favorites` checks before its per-track row reads.
    pub fn love_sync_active(&self) -> bool {
        let runtime = self.runtime.read();
        runtime.lastfm_love_armed() || runtime.listenbrainz_love_armed()
    }

    pub async fn enqueue_love(&self, row: &ScrobbleRow, loved: bool) -> AppResult<()> {
        self.enqueue_loves(std::slice::from_ref(row), loved).await
    }

    /// Queues a favorite toggle's loves, or unloves, under one lock, save and wake rather than
    /// one persist per row.
    pub async fn enqueue_loves(&self, rows: &[ScrobbleRow], loved: bool) -> AppResult<()> {
        let (lastfm_armed, listenbrainz_armed) = {
            let runtime = self.runtime.read();
            (runtime.lastfm_love_armed(), runtime.listenbrainz_love_armed())
        };
        if !lastfm_armed && !listenbrainz_armed {
            return Ok(());
        }
        let (pushed, snapshot) = {
            let mut queue = self.queue.lock();
            let mut pushed = 0usize;
            for row in rows {
                let Some(track) = ScrobbleTrack::from_row(row) else {
                    continue;
                };
                let listenbrainz_remaining = listenbrainz_armed && track.recording_mbid.is_some();
                if !lastfm_armed && !listenbrainz_remaining {
                    continue;
                }
                queue.push_love(LoveItem {
                    track,
                    loved,
                    lastfm_remaining: lastfm_armed,
                    listenbrainz_remaining,
                });
                pushed += 1;
            }
            (pushed, queue.clone())
        };
        if pushed == 0 {
            return Ok(());
        }
        self.save_queue(snapshot).await?;
        self.notify.notify_one();
        Ok(())
    }

    /// The gate the favorite backfill checks before fetching favorites.
    pub fn love_target_armed(&self, target: LoveTarget) -> bool {
        let runtime = self.runtime.read();
        match target {
            LoveTarget::Lastfm => runtime.lastfm_love_armed(),
            LoveTarget::ListenBrainz => runtime.listenbrainz_love_armed(),
        }
    }

    /// Queues a love for every favorite in `rows`, for `target` alone, returning how many were
    /// queued.
    pub async fn backfill_loves(
        &self,
        rows: &[ScrobbleRow],
        target: LoveTarget,
    ) -> AppResult<usize> {
        if !self.love_target_armed(target) {
            return Ok(0);
        }
        let (queued, snapshot) = {
            let mut queue = self.queue.lock();
            let mut queued = 0usize;
            for row in rows {
                let Some(track) = ScrobbleTrack::from_row(row) else {
                    continue;
                };
                let (lastfm_remaining, listenbrainz_remaining) = match target {
                    LoveTarget::Lastfm => (true, false),
                    LoveTarget::ListenBrainz if track.recording_mbid.is_some() => (false, true),
                    LoveTarget::ListenBrainz => continue,
                };
                queue.push_love(LoveItem {
                    track,
                    loved: true,
                    lastfm_remaining,
                    listenbrainz_remaining,
                });
                queued += 1;
            }
            (queued, queue.clone())
        };
        if queued == 0 {
            return Ok(0);
        }
        self.save_queue(snapshot).await?;
        self.notify.notify_one();
        Ok(queued)
    }

    fn client(&self) -> Client {
        self.http.get_or_init(build_http_client).clone()
    }

    /// Resolves at the next enqueue.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    /// The cheap gate the detector checks before its per-track row read.
    pub fn scrobbling_active(&self) -> bool {
        let runtime = self.runtime.read();
        runtime.lastfm_scrobble_ready() || runtime.listenbrainz_scrobble_ready()
    }

    /// Fire-and-forget on the ambient runtime, never queued or retried: a missed one costs nothing.
    pub fn update_now_playing(&self, track: ScrobbleTrack) {
        let (lastfm_creds, lb_creds) = {
            let runtime = self.runtime.read();
            (
                runtime.flags.lastfm_enabled.then(|| runtime.credentials.lastfm.clone()).flatten(),
                runtime
                    .flags
                    .listenbrainz_enabled
                    .then(|| runtime.credentials.listenbrainz.clone())
                    .flatten(),
            )
        };

        if let Some(creds) = lastfm_creds
            && let Some((api_key, secret)) = lastfm::keys()
        {
            let client = self.client();
            let track = track.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    lastfm::update_now_playing(&client, api_key, secret, &creds.session_key, &track)
                        .await
                {
                    log::debug!("Last.fm now-playing failed: {}", describe(&e));
                }
            });
        }

        if let Some(creds) = lb_creds {
            let client = self.client();
            let base = self.listenbrainz_base.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    listenbrainz::submit_playing_now(&client, &base, &creds.token, &track).await
                {
                    log::debug!("ListenBrainz now-playing failed: {}", describe(&e));
                }
            });
        }
    }

    /// A no-op when the row can't be scrobbled.
    pub async fn enqueue_scrobble(&self, row: &ScrobbleRow, timestamp: i64) -> AppResult<()> {
        match ScrobbleTrack::from_row(row) {
            Some(track) => self.enqueue_track(track, timestamp).await,
            None => Ok(()),
        }
    }

    /// Flags only the providers ready now, and is a no-op where none is.
    pub async fn enqueue_track(&self, track: ScrobbleTrack, timestamp: i64) -> AppResult<()> {
        let (lastfm_remaining, listenbrainz_remaining) = {
            let runtime = self.runtime.read();
            (runtime.lastfm_scrobble_ready(), runtime.listenbrainz_scrobble_ready())
        };
        if !lastfm_remaining && !listenbrainz_remaining {
            return Ok(());
        }
        self.push_scrobble(QueuedItem {
            track,
            timestamp,
            lastfm_remaining,
            listenbrainz_remaining,
        })
        .await?;
        self.notify.notify_one();
        Ok(())
    }
}

// Inline so the fixtures are a sibling the submit drain's suite reaches too; the bodies stay in
// their own files.
#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod helpers;

    #[path = "mod_tests.rs"]
    mod mod_tests;
}
