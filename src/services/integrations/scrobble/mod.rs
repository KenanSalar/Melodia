//! Scrobbling service: Last.fm + `ListenBrainz` "now playing" + scrobble
//! submission, fully decoupled from the player state machine.
//!
//! Owns the credential/enabled shadow and the durable offline queue (loaded at
//! boot, persisted on change), the submitter-wake `Notify`, and the shared lazy
//! `reqwest::Client`. `update_now_playing` / `enqueue_scrobble` / `submit_pending`
//! are driven by the [`detector`] state machine and the `tasks::scrobble` loops.

pub mod credentials;
pub mod detector;
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

use crate::config::Paths;
use crate::entities::integrations::ScrobbleFlags;
use crate::entities::track::ScrobbleRow;
use crate::error::{AppError, AppResult};
use crate::services::net::build_http_client;
use providers::lastfm;
use providers::listenbrainz;
use status::build_status;

/// In-memory shadow of the persisted credentials + enabled flags. Guarded by an
/// `RwLock` so the detector, submitter, and love-sync read connection/enabled
/// state synchronously without touching disk. The service's async methods drop
/// this guard before any `.await` (the disk write rides `spawn_blocking`), so the
/// lock is never held across an await point.
struct ScrobbleRuntime {
    credentials: ScrobbleCredentials,
    flags: ScrobbleFlags,
}

impl ScrobbleRuntime {
    /// Whether Last.fm could be reached right now: this build shipped app keys
    /// and a session is stored. Named `reachable` (not `connected`) to keep it
    /// distinct from [`ProviderStatus::connected`], which is creds-only.
    fn lastfm_reachable(&self) -> bool {
        lastfm::is_configured() && self.credentials.lastfm.is_some()
    }

    /// Whether `ListenBrainz` could be reached right now (needs no app keys —
    /// the per-user token is the whole credential).
    fn listenbrainz_reachable(&self) -> bool {
        self.credentials.listenbrainz.is_some()
    }

    /// Reachable **and** the Last.fm scrobble toggle is on.
    fn lastfm_scrobble_ready(&self) -> bool {
        self.lastfm_reachable() && self.flags.lastfm_enabled
    }

    /// Reachable **and** the `ListenBrainz` scrobble toggle is on.
    fn listenbrainz_scrobble_ready(&self) -> bool {
        self.listenbrainz_reachable() && self.flags.listenbrainz_enabled
    }

    /// Reachable **and** the Last.fm love toggle is on.
    fn lastfm_love_armed(&self) -> bool {
        self.lastfm_reachable() && self.flags.lastfm_love_enabled
    }

    /// Reachable **and** the `ListenBrainz` love toggle is on. Callers that queue
    /// a love add the per-track `recording_mbid.is_some()` gate themselves (LB
    /// feedback keys on it).
    fn listenbrainz_love_armed(&self) -> bool {
        self.listenbrainz_reachable() && self.flags.listenbrainz_love_enabled
    }
}

/// Owns the scrobble credential/enabled shadow and the durable offline queue.
/// Held as `Arc<ScrobbleService>` on [`crate::state::AppState`].
pub struct ScrobbleService {
    runtime: RwLock<ScrobbleRuntime>,
    queue: Mutex<ScrobbleQueue>,
    creds_path: PathBuf,
    queue_path: PathBuf,
    /// Wakes the submitter task when a scrobble is enqueued.
    notify: Notify,
    /// Wakes the MBID backfill task — on enabling auto-tagging or the manual
    /// "look up missing ids" button. Separate from `notify` so a scrobble
    /// enqueue doesn't spin the backfill and vice versa.
    mbid_kick: Notify,
    /// The same lazy `OnceLock` as [`crate::state::AppState`]'s client, so the
    /// whole app shares one connection pool built on first request.
    http: Arc<OnceLock<Client>>,
    /// Publishes a fresh [`ScrobbleStatus`] whenever the credentials or enabled
    /// flags change, so the settings UI stays truthful for connect/disconnect
    /// *and* the submitter's auto-disconnect (invalid Last.fm session /
    /// `ListenBrainz` token) without polling.
    status_tx: watch::Sender<ScrobbleStatus>,
}

impl ScrobbleService {
    /// Load credentials + queue from disk and seed the shadow with the enabled
    /// flags already read from `settings.json`. Infallible like
    /// [`crate::services::search_history::SearchHistoryState::init`]: a missing
    /// or corrupt file falls back to empty (logged) so boot never fails here.
    pub fn init(paths: &Paths, flags: &ScrobbleFlags, http: Arc<OnceLock<Client>>) -> Self {
        let credentials = credentials::load(&paths.scrobble_credentials_path).unwrap_or_default();
        let queue = ScrobbleQueue::load(&paths.scrobble_queue_path).unwrap_or_default();
        let (status_tx, _) = watch::channel(build_status(&credentials, flags));
        Self {
            runtime: RwLock::new(ScrobbleRuntime {
                credentials,
                flags: flags.clone(),
            }),
            queue: Mutex::new(queue),
            creds_path: paths.scrobble_credentials_path.clone(),
            queue_path: paths.scrobble_queue_path.clone(),
            notify: Notify::new(),
            mbid_kick: Notify::new(),
            http,
            status_tx,
        }
    }

    /// A cheap snapshot of connection + enabled state for the settings UI.
    pub fn status(&self) -> ScrobbleStatus {
        let runtime = self.runtime.read();
        build_status(&runtime.credentials, &runtime.flags)
    }

    /// Subscribe to status changes. The settings UI drives its rows off this so
    /// a background auto-disconnect is reflected without reopening the section.
    pub fn subscribe_status(&self) -> watch::Receiver<ScrobbleStatus> {
        self.status_tx.subscribe()
    }

    /// Mirror the enabled flags into the shadow after a setter has persisted
    /// them to `settings.json`, keeping synchronous readers current, then
    /// publish the fresh status.
    pub fn set_flags(&self, flags: ScrobbleFlags) {
        self.runtime.write().flags = flags;
        self.status_tx.send_replace(self.status());
    }

    /// Apply a credential change to the shadow, publish the fresh status, then
    /// persist the credential file — the shared body of the two public setters.
    /// The status is published from the in-memory shadow regardless of the disk
    /// write, so a failed persist still surfaces the applied state (matching the
    /// apply-then-persist convention elsewhere).
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
        // The credential JSON write is blocking `std::fs` (+ a chmod) — offload it
        // so a connect/disconnect never stalls a tokio worker. The shadow + status
        // are already applied above, so a failed write still surfaces the state.
        let path = self.creds_path.clone();
        match tokio::task::spawn_blocking(move || credentials::save(&path, &snapshot)).await {
            Ok(result) => result,
            Err(e) => {
                Err(AppError::io_other(format!("scrobble credential persist task panicked: {e}")))
            }
        }
    }

    /// Connect / disconnect Last.fm. Passing `None` disconnects.
    pub async fn set_lastfm_credentials(
        &self,
        credentials: Option<LastfmCredentials>,
    ) -> AppResult<()> {
        self.persist_credentials_change(move |creds| creds.lastfm = credentials).await
    }

    /// Connect / disconnect `ListenBrainz`. Passing `None` disconnects.
    pub async fn set_listenbrainz_credentials(
        &self,
        credentials: Option<ListenBrainzCredentials>,
    ) -> AppResult<()> {
        self.persist_credentials_change(move |creds| creds.listenbrainz = credentials).await
    }

    /// The `ListenBrainz` token to use for MBID lookups — `Some` only when
    /// auto-tagging is enabled **and** `ListenBrainz` is connected (the lookup
    /// endpoint requires the user's token). Read synchronously from the shadow.
    pub fn mbid_lookup_token(&self) -> Option<String> {
        let runtime = self.runtime.read();
        if runtime.flags.mbid_auto_tag {
            runtime.credentials.listenbrainz.as_ref().map(|c| c.token.clone())
        } else {
            None
        }
    }

    /// Wake the MBID backfill task to (re-)sweep the library — used by the
    /// enable toggle and the manual "look up missing ids" button.
    pub fn kick_mbid_backfill(&self) {
        self.mbid_kick.notify_one();
    }

    /// Await the next MBID backfill kick.
    pub async fn mbid_kicked(&self) {
        self.mbid_kick.notified().await;
    }

    /// Number of listens + loves still queued for submission. The submitter loop
    /// re-drains while this is non-zero, so loves must count here too.
    pub fn queued_len(&self) -> usize {
        let queue = self.queue.lock();
        queue.items.len() + queue.loves.len()
    }

    /// Append a scrobble to the durable queue and persist it. The submitter's
    /// `enqueue_scrobble` enriches and wakes on top of this primitive.
    pub async fn push_scrobble(&self, item: QueuedItem) -> AppResult<()> {
        let snapshot = {
            let mut queue = self.queue.lock();
            queue.push(item);
            queue.clone()
        };
        self.save_queue(snapshot).await
    }

    /// Persist a queue snapshot off the async runtime: the JSON write is blocking
    /// `std::fs`, so it rides `spawn_blocking` rather than stalling a tokio worker.
    /// The in-memory mutation already happened under the lock by the time this is
    /// called, so the snapshot is owned and `self` isn't captured. Propagates the
    /// write result; `submit::persist_queue` is the best-effort (log-only) wrapper.
    async fn save_queue(&self, snapshot: ScrobbleQueue) -> AppResult<()> {
        let path = self.queue_path.clone();
        match tokio::task::spawn_blocking(move || snapshot.save(&path)).await {
            Ok(result) => result,
            Err(e) => Err(AppError::io_other(format!("scrobble queue persist task panicked: {e}"))),
        }
    }

    /// Whether a favorite toggle should be mirrored to any provider right now —
    /// the cheap gate `library::favorites` checks before doing per-track DB
    /// lookups. True when at least one provider's love toggle is on and can
    /// receive a love: Last.fm (love on + this build shipped keys + connected) or
    /// `ListenBrainz` (love on + connected).
    pub fn love_sync_active(&self) -> bool {
        let runtime = self.runtime.read();
        runtime.lastfm_love_armed() || runtime.listenbrainz_love_armed()
    }

    /// Enrich a single DB row into a durable love/unlove and wake the submitter —
    /// the one-track case of [`Self::enqueue_loves`], which owns the per-provider
    /// arming and the `ListenBrainz` MBID gate. A no-op when the row can't be built
    /// or no provider wants it.
    pub async fn enqueue_love(&self, row: &ScrobbleRow, loved: bool) -> AppResult<()> {
        self.enqueue_loves(std::slice::from_ref(row), loved).await
    }

    /// Queue a love/unlove for a multi-track favorite toggle under a **single**
    /// queue lock + save + submitter wake, instead of one persist per id — the
    /// primitive [`Self::enqueue_love`] delegates its single-row case to. Arms a
    /// provider per row only when it can receive the love: Last.fm when its love
    /// toggle is armed; `ListenBrainz` additionally gated on a `recording_mbid`
    /// (LB feedback keys on it, so untagged tracks skip LB). `push_love` still
    /// coalesces a repeat toggle of the same track; `loved` carries un-favorites
    /// too. A no-op when love-sync is off or no row wants any provider.
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
                // LB feedback keys on the recording MBID — skip untagged tracks.
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

    /// Whether `target`'s love toggle can receive loves right now — the cheap
    /// gate the retroactive favorite backfill checks before fetching favorites.
    pub fn love_target_armed(&self, target: LoveTarget) -> bool {
        let runtime = self.runtime.read();
        match target {
            LoveTarget::Lastfm => runtime.lastfm_love_armed(),
            LoveTarget::ListenBrainz => runtime.listenbrainz_love_armed(),
        }
    }

    /// Retroactively queue a love for every favorite `row`, armed for **only**
    /// `target` (so connecting one provider doesn't re-love everything on the
    /// other). Batches the whole set into a single queue lock + save + submitter
    /// wake, unlike per-track [`Self::enqueue_love`]. `ListenBrainz` skips rows
    /// without a `recording_mbid` (it keys on that). Returns how many loves were
    /// actually queued. A no-op returning `Ok(0)` when `target` isn't armed.
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
                    // LB feedback keys on the recording MBID — skip untagged tracks.
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

    /// The shared lazy `reqwest::Client`, built on first use. Clones are cheap —
    /// a `reqwest::Client` is `Arc`-backed and shares the connection pool.
    fn client(&self) -> Client {
        self.http.get_or_init(build_http_client).clone()
    }

    /// Await the next submitter wake (a scrobble was enqueued).
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    /// Fire-and-forget "now playing" to every connected + enabled provider.
    /// Ephemeral — never queued or retried, since a missed update is
    /// inconsequential. Reads the shadow synchronously (never held across
    /// `.await`), then spawns one POST per provider on the ambient runtime.
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
            && let (Some(api_key), Some(secret)) =
                (lastfm::LASTFM_API_KEY, lastfm::LASTFM_SHARED_SECRET)
        {
            let client = self.client();
            let track = track.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    lastfm::update_now_playing(&client, api_key, secret, &creds.session_key, &track)
                        .await
                {
                    log::debug!("Last.fm now-playing failed: {e}");
                }
            });
        }

        if let Some(creds) = lb_creds {
            let client = self.client();
            tokio::spawn(async move {
                if let Err(e) =
                    listenbrainz::submit_playing_now(&client, &creds.token, &track).await
                {
                    log::debug!("ListenBrainz now-playing failed: {e}");
                }
            });
        }
    }

    /// Enrich a DB row into a durable scrobble and wake the submitter. Sets a
    /// provider's "needs submitting" flag only when that provider is currently
    /// connected + enabled (a flag for a disconnected provider would never clear
    /// and would pin the item through `retain_pending`). A no-op when the row
    /// can't be scrobbled or no provider wants it.
    pub async fn enqueue_scrobble(&self, row: &ScrobbleRow, timestamp: i64) -> AppResult<()> {
        let Some(track) = ScrobbleTrack::from_row(row) else {
            return Ok(());
        };
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

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
