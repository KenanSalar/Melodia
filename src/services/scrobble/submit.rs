//! The submitter's drain passes: batched scrobble submission and per-item love
//! submission, with per-provider readiness gating, auto-disconnect on rejected
//! auth, and rate-limit/transient backoff. Split out of the [`ScrobbleService`]
//! handle (`mod.rs`) so the HTTP-drain concern stays separate from the state
//! handle; it reaches the service's private queue/shadow as a descendant module.

use std::collections::VecDeque;
use std::time::Duration;

use super::ScrobbleService;
use super::model::ScrobbleTrack;
use super::providers::lastfm::{self, LastfmError};
use super::providers::listenbrainz::{self, ListenBrainzError};
use super::queue::{LoveItem, ProviderFlags, QueuedItem, ScrobbleQueue};

/// Provider cap on listens per submission POST (Last.fm's limit; we share it for
/// `ListenBrainz` too, which permits more).
const SCROBBLE_BATCH_MAX: usize = 50;

impl ScrobbleService {
    /// Log the Last.fm session rejection and clear the stored credential
    /// (best-effort persist). Called from the submit drains on an invalid session
    /// — the caller then drops the pending Last.fm flags itself.
    fn disconnect_lastfm(&self) {
        log::warn!("Last.fm session invalid; disconnecting");
        if let Err(e) = self.set_lastfm_credentials(None) {
            log::warn!("Failed to persist Last.fm disconnect: {e}");
        }
    }

    /// `ListenBrainz` sibling of [`Self::disconnect_lastfm`], on a rejected token.
    fn disconnect_listenbrainz(&self) {
        log::warn!("ListenBrainz token invalid; disconnecting");
        if let Err(e) = self.set_listenbrainz_credentials(None) {
            log::warn!("Failed to persist ListenBrainz disconnect: {e}");
        }
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
        // Readiness (scrobble toggle on + reachable) is snapshotted alongside the
        // creds so both come from the same lock acquisition.
        let (lastfm_creds, lastfm_ready, lb_creds, lb_ready) = {
            let runtime = self.runtime.read();
            (
                runtime.credentials.lastfm.clone(),
                runtime.lastfm_scrobble_ready(),
                runtime.credentials.listenbrainz.clone(),
                runtime.listenbrainz_scrobble_ready(),
            )
        };

        let client = self.client();
        let mut clear_lastfm: Vec<usize> = Vec::new();
        let mut clear_lb: Vec<usize> = Vec::new();
        let mut retry_after: Option<Duration> = None;

        // ---- Last.fm ----
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
                        self.disconnect_lastfm();
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
        if !lb_ready {
            drop_flags(&snapshot, |it| it.listenbrainz_remaining, &mut clear_lb);
        } else if let Some(creds) = lb_creds.as_ref() {
            let (batch, idx) = take_batch(&snapshot, |it| it.listenbrainz_remaining);
            if !batch.is_empty() {
                match listenbrainz::submit_listens(&client, &creds.token, &batch).await {
                    Ok(()) => clear_lb.extend(idx),
                    Err(ListenBrainzError::InvalidToken) => {
                        self.disconnect_listenbrainz();
                        drop_flags(&snapshot, |it| it.listenbrainz_remaining, &mut clear_lb);
                    }
                    Err(ListenBrainzError::RateLimited { reset_in_secs }) => {
                        let d = listenbrainz::rate_limit_backoff(reset_in_secs);
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

        self.apply_writeback(|q| &mut q.items, &clear_lastfm, &clear_lb);
        retry_after
    }

    /// Clear the submitted providers' flags by snapshot index (bounds-checked
    /// against a rare cap-drop shift — a double-submit is deduped by both
    /// services), `retain_pending`, and persist only when the queue changed.
    /// `select` picks the sub-queue to write back — `items` for scrobbles,
    /// `loves` for loves — so both drains share this one walk.
    fn apply_writeback<T: ProviderFlags>(
        &self,
        select: impl Fn(&mut ScrobbleQueue) -> &mut VecDeque<T>,
        clear_lastfm: &[usize],
        clear_lb: &[usize],
    ) {
        let to_save: Option<ScrobbleQueue> = {
            let mut queue = self.queue.lock();
            let before = select(&mut queue).len();
            for &i in clear_lastfm {
                if let Some(item) = select(&mut queue).get_mut(i) {
                    item.set_lastfm_remaining(false);
                }
            }
            for &i in clear_lb {
                if let Some(item) = select(&mut queue).get_mut(i) {
                    item.set_listenbrainz_remaining(false);
                }
            }
            queue.retain_pending();
            let changed = !clear_lastfm.is_empty()
                || !clear_lb.is_empty()
                || select(&mut queue).len() != before;
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

        // Loves drain on reachability alone — the love toggles (not the scrobble
        // toggles) gate *enqueuing*, so neither `*_enabled` flag is read here.
        let (lastfm_creds, lastfm_ready, lb_creds, lb_ready) = {
            let runtime = self.runtime.read();
            (
                runtime.credentials.lastfm.clone(),
                runtime.lastfm_reachable(),
                runtime.credentials.listenbrainz.clone(),
                runtime.listenbrainz_reachable(),
            )
        };

        let client = self.client();
        let mut clear_lastfm: Vec<usize> = Vec::new();
        let mut clear_lb: Vec<usize> = Vec::new();
        let mut retry_after: Option<Duration> = None;

        // ---- Last.fm ----
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
                        self.disconnect_lastfm();
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
                        self.disconnect_listenbrainz();
                        drop_flags(&snapshot, |it| it.listenbrainz_remaining, &mut clear_lb);
                        break;
                    }
                    Err(ListenBrainzError::RateLimited { reset_in_secs }) => {
                        let d = listenbrainz::rate_limit_backoff(reset_in_secs);
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

        self.apply_writeback(|q| &mut q.loves, &clear_lastfm, &clear_lb);
        retry_after
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
