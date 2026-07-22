//! Scrobbling service: Last.fm + `ListenBrainz` "now playing" + scrobble
//! submission, fully decoupled from the player state machine.
//!
//! Owns the credential/enabled shadow and the durable offline queue (loaded at
//! boot, persisted on change), the submitter-wake `Notify`, and the shared lazy
//! `reqwest::Client`. `update_now_playing` / `enqueue_scrobble` / `submit_pending`
//! are driven by the [`detector`] state machine and the `tasks::scrobble` loops;
//! only the settings UI and love↔favorite sync wire in later phases.

pub mod credentials;
pub mod detector;
pub mod model;
pub mod providers;
pub mod queue;

pub use credentials::{LastfmCredentials, ListenBrainzCredentials, ScrobbleCredentials};
pub use model::{ScrobbleTrack, scrobble_threshold_ms};
pub use queue::{LoveItem, QueuedItem, ScrobbleQueue};

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use reqwest::Client;
use tokio::sync::{Notify, watch};

use crate::config::Paths;
use crate::entities::track::ScrobbleRow;
use crate::error::AppResult;
use crate::services::build_http_client;
use crate::services::settings::ScrobbleFlags;
use providers::lastfm::{self, LastfmError};
use providers::listenbrainz::{self, ListenBrainzError};

/// Provider cap on listens per submission POST (Last.fm's limit; we share it for
/// `ListenBrainz` too, which permits more).
const SCROBBLE_BATCH_MAX: usize = 50;

/// Retry delay for a `ListenBrainz` rate limit that arrived without a
/// `X-RateLimit-Reset-In` header to honor.
const DEFAULT_RATE_LIMIT_BACKOFF_SECS: u64 = 30;

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
    pub mbid_auto_tag: bool,
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

/// Build a [`ScrobbleStatus`] snapshot from the raw shadow parts. Shared by
/// `status()` and `init` (which needs it before `Self` exists).
fn build_status(credentials: &ScrobbleCredentials, flags: &ScrobbleFlags) -> ScrobbleStatus {
    ScrobbleStatus {
        lastfm: ProviderStatus {
            connected: credentials.lastfm.is_some(),
            username: credentials.lastfm.as_ref().map(|c| c.username.clone()),
            enabled: flags.lastfm_enabled,
        },
        listenbrainz: ProviderStatus {
            connected: credentials.listenbrainz.is_some(),
            username: credentials.listenbrainz.as_ref().map(|c| c.username.clone()),
            enabled: flags.listenbrainz_enabled,
        },
        love_sync_enabled: flags.love_sync_enabled,
        mbid_auto_tag: flags.mbid_auto_tag,
    }
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

    /// Connect / disconnect Last.fm: update the shadow, publish the status, then
    /// persist the credential file. Passing `None` disconnects. The status is
    /// published from the in-memory shadow regardless of the disk write, so a
    /// failed persist still surfaces the applied state (matching the
    /// apply-then-persist convention elsewhere).
    pub fn set_lastfm_credentials(&self, credentials: Option<LastfmCredentials>) -> AppResult<()> {
        let snapshot = {
            let mut runtime = self.runtime.write();
            runtime.credentials.lastfm = credentials;
            runtime.credentials.clone()
        };
        self.status_tx.send_replace(self.status());
        credentials::save(&self.creds_path, &snapshot)
    }

    /// Connect / disconnect `ListenBrainz`: update the shadow, publish the
    /// status, then persist the credential file. Passing `None` disconnects.
    pub fn set_listenbrainz_credentials(
        &self,
        credentials: Option<ListenBrainzCredentials>,
    ) -> AppResult<()> {
        let snapshot = {
            let mut runtime = self.runtime.write();
            runtime.credentials.listenbrainz = credentials;
            runtime.credentials.clone()
        };
        self.status_tx.send_replace(self.status());
        credentials::save(&self.creds_path, &snapshot)
    }

    /// The `ListenBrainz` token to use for MBID lookups — `Some` only when
    /// auto-tagging is enabled **and** `ListenBrainz` is connected (the lookup
    /// endpoint requires the user's token). Read synchronously from the shadow.
    pub fn mbid_lookup_token(&self) -> Option<String> {
        let runtime = self.runtime.read();
        if runtime.flags.mbid_auto_tag {
            runtime
                .credentials
                .listenbrainz
                .as_ref()
                .map(|c| c.token.clone())
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

    /// Append a love/unlove to the durable queue (coalescing a repeat toggle of
    /// the same track) and persist it. `enqueue_love` gates and enriches on top.
    fn push_love(&self, item: LoveItem) -> AppResult<()> {
        let snapshot = {
            let mut queue = self.queue.lock();
            queue.push_love(item);
            queue.clone()
        };
        snapshot.save(&self.queue_path)
    }

    /// Whether a favorite toggle should be mirrored to any provider right now —
    /// the cheap gate `library::favorites` checks before doing per-track DB
    /// lookups. True only when love-sync is on and at least one provider can
    /// receive a love: Last.fm (this build shipped keys + connected) or
    /// `ListenBrainz` (enabled + connected).
    pub fn love_sync_active(&self) -> bool {
        let runtime = self.runtime.read();
        runtime.flags.love_sync_enabled
            && ((lastfm::is_configured() && runtime.credentials.lastfm.is_some())
                || (runtime.flags.listenbrainz_enabled
                    && runtime.credentials.listenbrainz.is_some()))
    }

    /// Enrich a DB row into a durable love/unlove and wake the submitter. Gated
    /// on `love_sync_enabled`; sets a provider's flag only when it can receive
    /// the love — Last.fm when configured + connected, `ListenBrainz` when
    /// enabled + connected **and** the track carries a `recording_mbid` (LB
    /// feedback keys on it, so untagged tracks skip LB). A no-op when the row
    /// can't be built or no provider wants it.
    pub fn enqueue_love(&self, row: &ScrobbleRow, loved: bool) -> AppResult<()> {
        let Some(track) = ScrobbleTrack::from_row(row) else {
            return Ok(());
        };
        let (lastfm_remaining, listenbrainz_remaining) = {
            let runtime = self.runtime.read();
            if runtime.flags.love_sync_enabled {
                (
                    lastfm::is_configured() && runtime.credentials.lastfm.is_some(),
                    runtime.flags.listenbrainz_enabled
                        && runtime.credentials.listenbrainz.is_some()
                        && track.recording_mbid.is_some(),
                )
            } else {
                (false, false)
            }
        };
        if !lastfm_remaining && !listenbrainz_remaining {
            return Ok(());
        }
        self.push_love(LoveItem {
            track,
            loved,
            lastfm_remaining,
            listenbrainz_remaining,
        })?;
        self.notify.notify_one();
        Ok(())
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
                runtime
                    .flags
                    .lastfm_enabled
                    .then(|| runtime.credentials.lastfm.clone())
                    .flatten(),
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
                if let Err(e) = listenbrainz::submit_playing_now(&client, &creds.token, &track).await
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
    pub fn enqueue_scrobble(&self, row: &ScrobbleRow, timestamp: i64) -> AppResult<()> {
        let Some(track) = ScrobbleTrack::from_row(row) else {
            return Ok(());
        };
        let (lastfm_remaining, listenbrainz_remaining) = {
            let runtime = self.runtime.read();
            (
                lastfm::is_configured()
                    && runtime.flags.lastfm_enabled
                    && runtime.credentials.lastfm.is_some(),
                runtime.flags.listenbrainz_enabled && runtime.credentials.listenbrainz.is_some(),
            )
        };
        if !lastfm_remaining && !listenbrainz_remaining {
            return Ok(());
        }
        self.push_scrobble(QueuedItem {
            track,
            timestamp,
            lastfm_remaining,
            listenbrainz_remaining,
        })?;
        self.notify.notify_one();
        Ok(())
    }

    /// One drain round over both the scrobble and love queues. Returns
    /// `Some(delay)` when a provider asked to be retried later (transient / rate
    /// limit); `None` when idle or progress was made. Routine failures stay
    /// silent (logged), per the no-toast-spam convention.
    pub async fn submit_pending(&self) -> Option<Duration> {
        let (has_items, has_loves) = {
            let queue = self.queue.lock();
            (!queue.items.is_empty(), !queue.loves.is_empty())
        };
        if !has_items && !has_loves {
            return None;
        }
        let mut retry = None;
        if has_items {
            retry = merge_opt(retry, self.submit_scrobbles().await);
        }
        if has_loves {
            retry = merge_opt(retry, self.submit_loves().await);
        }
        retry
    }

    /// Drain the scrobble queue: batch each connected + enabled provider (≤
    /// `SCROBBLE_BATCH_MAX`), POST via the Phase-1 clients, clear the
    /// per-provider flag on success, drop the flag for a now-disconnected
    /// provider, then `retain_pending` + persist.
    async fn submit_scrobbles(&self) -> Option<Duration> {
        let snapshot: Vec<QueuedItem> = {
            let queue = self.queue.lock();
            if queue.items.is_empty() {
                return None;
            }
            queue.items.iter().cloned().collect()
        };

        // One shadow read; secrets cloned out so no guard is held across a POST.
        let (lastfm_creds, lastfm_enabled, lb_creds, lb_enabled) = {
            let runtime = self.runtime.read();
            (
                runtime.credentials.lastfm.clone(),
                runtime.flags.lastfm_enabled,
                runtime.credentials.listenbrainz.clone(),
                runtime.flags.listenbrainz_enabled,
            )
        };

        let client = self.client();
        let mut clear_lastfm: Vec<usize> = Vec::new();
        let mut clear_lb: Vec<usize> = Vec::new();
        let mut retry_after: Option<Duration> = None;

        // ---- Last.fm ----
        let lastfm_ready = lastfm::is_configured() && lastfm_enabled && lastfm_creds.is_some();
        if !lastfm_ready {
            // Nowhere to send: drop the Last.fm side of every pending item.
            drop_flags(&snapshot, |it| it.lastfm_remaining, &mut clear_lastfm);
        } else if let Some(creds) = lastfm_creds.as_ref()
            && let (Some(api_key), Some(secret)) =
                (lastfm::LASTFM_API_KEY, lastfm::LASTFM_SHARED_SECRET)
        {
            let (batch, idx) = take_batch(&snapshot, |it| it.lastfm_remaining);
            if !batch.is_empty() {
                match lastfm::scrobble_batch(&client, api_key, secret, &creds.session_key, &batch)
                    .await
                {
                    Ok(()) => clear_lastfm.extend(idx),
                    Err(LastfmError::InvalidSession) => {
                        log::warn!("Last.fm session invalid; disconnecting");
                        if let Err(e) = self.set_lastfm_credentials(None) {
                            log::warn!("Failed to persist Last.fm disconnect: {e}");
                        }
                        drop_flags(&snapshot, |it| it.lastfm_remaining, &mut clear_lastfm);
                    }
                    Err(e) => {
                        log::info!("Last.fm scrobble deferred: {e}");
                        retry_after = Some(merge_retry(retry_after, Duration::ZERO));
                    }
                }
            }
        }

        // ---- ListenBrainz ----
        let lb_ready = lb_enabled && lb_creds.is_some();
        if !lb_ready {
            drop_flags(&snapshot, |it| it.listenbrainz_remaining, &mut clear_lb);
        } else if let Some(creds) = lb_creds.as_ref() {
            let (batch, idx) = take_batch(&snapshot, |it| it.listenbrainz_remaining);
            if !batch.is_empty() {
                match listenbrainz::submit_listens(&client, &creds.token, &batch).await {
                    Ok(()) => clear_lb.extend(idx),
                    Err(ListenBrainzError::InvalidToken) => {
                        log::warn!("ListenBrainz token invalid; disconnecting");
                        if let Err(e) = self.set_listenbrainz_credentials(None) {
                            log::warn!("Failed to persist ListenBrainz disconnect: {e}");
                        }
                        drop_flags(&snapshot, |it| it.listenbrainz_remaining, &mut clear_lb);
                    }
                    Err(ListenBrainzError::RateLimited { reset_in_secs }) => {
                        let d = Duration::from_secs(
                            reset_in_secs.unwrap_or(DEFAULT_RATE_LIMIT_BACKOFF_SECS),
                        );
                        log::info!("ListenBrainz rate limited; retrying in {}s", d.as_secs());
                        retry_after = Some(merge_retry(retry_after, d));
                    }
                    Err(e) => {
                        log::info!("ListenBrainz submit deferred: {e}");
                        retry_after = Some(merge_retry(retry_after, Duration::ZERO));
                    }
                }
            }
        }

        self.apply_writeback(&clear_lastfm, &clear_lb);
        retry_after
    }

    /// Clear the submitted providers' flags by snapshot index (bounds-checked
    /// against a rare cap-drop shift — a double-submit is deduped by both
    /// services), `retain_pending`, and persist only when the queue changed.
    fn apply_writeback(&self, clear_lastfm: &[usize], clear_lb: &[usize]) {
        let to_save: Option<ScrobbleQueue> = {
            let mut queue = self.queue.lock();
            let before = queue.items.len();
            for &i in clear_lastfm {
                if let Some(item) = queue.items.get_mut(i) {
                    item.lastfm_remaining = false;
                }
            }
            for &i in clear_lb {
                if let Some(item) = queue.items.get_mut(i) {
                    item.listenbrainz_remaining = false;
                }
            }
            queue.retain_pending();
            let changed =
                !clear_lastfm.is_empty() || !clear_lb.is_empty() || queue.items.len() != before;
            changed.then(|| queue.clone())
        };
        if let Some(snapshot) = to_save
            && let Err(e) = snapshot.save(&self.queue_path)
        {
            log::warn!("Failed to persist scrobble queue after submit: {e}");
        }
    }

    /// Drain the love queue: one POST per pending love (Last.fm
    /// `track.love`/`track.unlove`, `ListenBrainz` recording feedback), clearing
    /// the per-provider flag on success and auto-disconnecting on rejected auth —
    /// mirroring `submit_scrobbles`. Reads the shadow fresh so a disconnect from
    /// the scrobble pass in the same round is honored. Capped per round; the
    /// submitter loop re-drains while loves remain.
    async fn submit_loves(&self) -> Option<Duration> {
        let snapshot: Vec<LoveItem> = {
            let queue = self.queue.lock();
            if queue.loves.is_empty() {
                return None;
            }
            queue.loves.iter().cloned().collect()
        };

        let (lastfm_creds, lb_creds, lb_enabled) = {
            let runtime = self.runtime.read();
            (
                runtime.credentials.lastfm.clone(),
                runtime.credentials.listenbrainz.clone(),
                runtime.flags.listenbrainz_enabled,
            )
        };

        let client = self.client();
        let mut clear_lastfm: Vec<usize> = Vec::new();
        let mut clear_lb: Vec<usize> = Vec::new();
        let mut retry_after: Option<Duration> = None;

        // ---- Last.fm ----
        let lastfm_ready = lastfm::is_configured() && lastfm_creds.is_some();
        if !lastfm_ready {
            drop_flags(&snapshot, |it| it.lastfm_remaining, &mut clear_lastfm);
        } else if let Some(creds) = lastfm_creds.as_ref()
            && let (Some(api_key), Some(secret)) =
                (lastfm::LASTFM_API_KEY, lastfm::LASTFM_SHARED_SECRET)
        {
            for (i, love) in snapshot.iter().enumerate() {
                if clear_lastfm.len() >= SCROBBLE_BATCH_MAX {
                    break;
                }
                if !love.lastfm_remaining {
                    continue;
                }
                match lastfm::love(
                    &client,
                    api_key,
                    secret,
                    &creds.session_key,
                    &love.track,
                    love.loved,
                )
                .await
                {
                    Ok(()) => clear_lastfm.push(i),
                    Err(LastfmError::InvalidSession) => {
                        log::warn!("Last.fm session invalid; disconnecting");
                        if let Err(e) = self.set_lastfm_credentials(None) {
                            log::warn!("Failed to persist Last.fm disconnect: {e}");
                        }
                        drop_flags(&snapshot, |it| it.lastfm_remaining, &mut clear_lastfm);
                        break;
                    }
                    Err(e) => {
                        log::info!("Last.fm love deferred: {e}");
                        retry_after = Some(merge_retry(retry_after, Duration::ZERO));
                        break;
                    }
                }
            }
        }

        // ---- ListenBrainz ----
        let lb_ready = lb_enabled && lb_creds.is_some();
        if !lb_ready {
            drop_flags(&snapshot, |it| it.listenbrainz_remaining, &mut clear_lb);
        } else if let Some(creds) = lb_creds.as_ref() {
            for (i, love) in snapshot.iter().enumerate() {
                if clear_lb.len() >= SCROBBLE_BATCH_MAX {
                    break;
                }
                if !love.listenbrainz_remaining {
                    continue;
                }
                let Some(mbid) = love.track.recording_mbid.as_deref() else {
                    clear_lb.push(i); // no MBID for LB to key on → nothing to do
                    continue;
                };
                match listenbrainz::submit_feedback(&client, &creds.token, mbid, i8::from(love.loved))
                    .await
                {
                    Ok(()) => clear_lb.push(i),
                    Err(ListenBrainzError::InvalidToken) => {
                        log::warn!("ListenBrainz token invalid; disconnecting");
                        if let Err(e) = self.set_listenbrainz_credentials(None) {
                            log::warn!("Failed to persist ListenBrainz disconnect: {e}");
                        }
                        drop_flags(&snapshot, |it| it.listenbrainz_remaining, &mut clear_lb);
                        break;
                    }
                    Err(ListenBrainzError::RateLimited { reset_in_secs }) => {
                        let d = Duration::from_secs(
                            reset_in_secs.unwrap_or(DEFAULT_RATE_LIMIT_BACKOFF_SECS),
                        );
                        log::info!("ListenBrainz rate limited; retrying in {}s", d.as_secs());
                        retry_after = Some(merge_retry(retry_after, d));
                        break;
                    }
                    Err(e) => {
                        log::info!("ListenBrainz feedback deferred: {e}");
                        retry_after = Some(merge_retry(retry_after, Duration::ZERO));
                        break;
                    }
                }
            }
        }

        self.apply_love_writeback(&clear_lastfm, &clear_lb);
        retry_after
    }

    /// Clear the submitted providers' love flags by snapshot index,
    /// `retain_pending`, and persist only when the love queue changed.
    fn apply_love_writeback(&self, clear_lastfm: &[usize], clear_lb: &[usize]) {
        let to_save: Option<ScrobbleQueue> = {
            let mut queue = self.queue.lock();
            let before = queue.loves.len();
            for &i in clear_lastfm {
                if let Some(item) = queue.loves.get_mut(i) {
                    item.lastfm_remaining = false;
                }
            }
            for &i in clear_lb {
                if let Some(item) = queue.loves.get_mut(i) {
                    item.listenbrainz_remaining = false;
                }
            }
            queue.retain_pending();
            let changed =
                !clear_lastfm.is_empty() || !clear_lb.is_empty() || queue.loves.len() != before;
            changed.then(|| queue.clone())
        };
        if let Some(snapshot) = to_save
            && let Err(e) = snapshot.save(&self.queue_path)
        {
            log::warn!("Failed to persist scrobble queue after love submit: {e}");
        }
    }
}

/// Fold a requested retry delay into the running maximum, so honoring several
/// providers means honoring the longest.
fn merge_retry(current: Option<Duration>, requested: Duration) -> Duration {
    current.map_or(requested, |c| c.max(requested))
}

/// Combine two optional retry delays, keeping the longer — the scrobble and love
/// drains each report one, and the loop honors whichever asks to wait longest.
fn merge_opt(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (some, None) | (None, some) => some,
    }
}

/// Collect the snapshot indices whose `flag` predicate holds — the entries to
/// mark done for a provider that can't be submitted to right now. Generic over
/// the queued type, shared by the scrobble and love drains.
fn drop_flags<T>(snapshot: &[T], flag: impl Fn(&T) -> bool, out: &mut Vec<usize>) {
    out.extend(
        snapshot
            .iter()
            .enumerate()
            .filter(|(_, it)| flag(it))
            .map(|(i, _)| i),
    );
}

/// Build a `≤ SCROBBLE_BATCH_MAX` batch of `(&track, timestamp)` from the items
/// whose `flag` is set, returning it alongside their snapshot indices.
fn take_batch(
    snapshot: &[QueuedItem],
    flag: impl Fn(&QueuedItem) -> bool,
) -> (Vec<(&ScrobbleTrack, i64)>, Vec<usize>) {
    let mut batch: Vec<(&ScrobbleTrack, i64)> = Vec::new();
    let mut idx: Vec<usize> = Vec::new();
    for (i, item) in snapshot.iter().enumerate() {
        if batch.len() >= SCROBBLE_BATCH_MAX {
            break;
        }
        if flag(item) {
            batch.push((&item.track, item.timestamp));
            idx.push(i);
        }
    }
    (batch, idx)
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
