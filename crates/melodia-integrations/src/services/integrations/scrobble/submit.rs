//! The submitter's drain passes: batched scrobble submission and per-item love
//! submission, with per-provider readiness gating, auto-disconnect on rejected
//! auth, and rate-limit/transient backoff. Split out of the [`ScrobbleService`]
//! handle (`mod.rs`) so the HTTP-drain concern stays separate from the state
//! handle; it reaches the service's private queue/shadow as a descendant module.

use std::collections::VecDeque;
use std::time::Duration;

use reqwest::Client;
use tokio_util::sync::CancellationToken;

use melodia_core::error::describe;

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
    async fn disconnect_lastfm(&self) {
        log::warn!("Last.fm session invalid; disconnecting");
        if let Err(e) = self.set_lastfm_credentials(None).await {
            log::warn!("Failed to persist Last.fm disconnect: {}", describe(&e));
        }
    }

    /// `ListenBrainz` sibling of [`Self::disconnect_lastfm`], on a rejected token.
    async fn disconnect_listenbrainz(&self) {
        log::warn!("ListenBrainz token invalid; disconnecting");
        if let Err(e) = self.set_listenbrainz_credentials(None).await {
            log::warn!("Failed to persist ListenBrainz disconnect: {}", describe(&e));
        }
    }

    /// One drain round over both the scrobble and love queues. Returns
    /// `Some(delay)` when a provider asked to be retried later (transient / rate
    /// limit); `None` when idle or progress was made. Routine failures stay
    /// silent (logged), per the no-toast-spam convention.
    ///
    /// Once `stop` fires no further request starts and the one in flight is
    /// abandoned, but what already went out is still written back: a round cut
    /// short otherwise re-sends all of it next time.
    pub async fn submit_pending(&self, stop: &CancellationToken) -> Option<Duration> {
        let (has_items, has_loves) = {
            let queue = self.queue.lock();
            (!queue.items.is_empty(), !queue.loves.is_empty())
        };
        if !has_items && !has_loves {
            return None;
        }
        let mut retry = None;
        if has_items {
            retry = merge_opt(retry, self.submit_scrobbles(stop).await);
        }
        if has_loves {
            retry = merge_opt(retry, self.submit_loves(stop).await);
        }
        retry
    }

    /// Drain the scrobble queue: batch each connected + enabled provider (≤
    /// `SCROBBLE_BATCH_MAX`), POST via the Phase-1 clients, clear the
    /// per-provider flag on success, drop the flag for a now-disconnected
    /// provider, then `retain_pending` + persist.
    async fn submit_scrobbles(&self, stop: &CancellationToken) -> Option<Duration> {
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
        let mut lastfm = Drain::new(&snapshot, |it: &QueuedItem| it.lastfm_remaining);
        if !lastfm_ready {
            // Nowhere to send: drop the Last.fm side of every pending item.
            lastfm.drop_pending();
        } else if let Some(creds) = lastfm_creds.as_ref()
            && let (Some(api_key), Some(secret)) =
                (lastfm::LASTFM_API_KEY, lastfm::LASTFM_SHARED_SECRET)
        {
            let session = LastfmSession { api_key, secret, session_key: &creds.session_key };
            self.scrobble_to_lastfm(&mut lastfm, &client, &session, stop).await;
        } else {
            // `lastfm_ready` implies `is_configured()` (both keys present) AND a
            // stored session, so this arm is unreachable. If a future change ever
            // decouples that invariant, drop the flags rather than leave them set
            // with no retry — which would spin the submitter at zero delay.
            debug_assert!(
                false,
                "lastfm_ready but api keys/session absent — is_configured() invariant broke"
            );
            lastfm.drop_pending();
        }

        let mut lb = Drain::new(&snapshot, |it: &QueuedItem| it.listenbrainz_remaining);
        if !lb_ready {
            lb.drop_pending();
        } else if let Some(creds) = lb_creds.as_ref() {
            self.scrobble_to_listenbrainz(&mut lb, &client, &creds.token, stop).await;
        }

        // Scrobbles never coalesce in place, so an unconditional clear is safe.
        if let Some(snapshot) =
            self.collect_writeback(|q| &mut q.items, &lastfm.done, &lb.done, |_, _| true)
        {
            self.persist_queue(snapshot).await;
        }
        merge_opt(lastfm.retry_after, lb.retry_after)
    }

    async fn scrobble_to_lastfm(
        &self,
        drain: &mut Drain<'_, QueuedItem>,
        client: &Client,
        session: &LastfmSession<'_>,
        stop: &CancellationToken,
    ) {
        let (batch, idx) = take_batch(drain.snapshot, |it| it.lastfm_remaining);
        if batch.is_empty() {
            return;
        }
        let LastfmSession { api_key, secret, session_key } = *session;
        match stop
            .run_until_cancelled(lastfm::scrobble_batch(
                client,
                api_key,
                secret,
                session_key,
                &batch,
            ))
            .await
        {
            None => {}
            Some(Ok(())) => drain.done.extend(idx),
            Some(Err(e)) => self.on_lastfm_error(&e, "Last.fm scrobble", drain).await,
        }
    }

    async fn scrobble_to_listenbrainz(
        &self,
        drain: &mut Drain<'_, QueuedItem>,
        client: &Client,
        token: &str,
        stop: &CancellationToken,
    ) {
        let (batch, idx) = take_batch(drain.snapshot, |it| it.listenbrainz_remaining);
        if batch.is_empty() {
            return;
        }
        match stop
            .run_until_cancelled(listenbrainz::submit_listens(
                client,
                &self.listenbrainz_base,
                token,
                &batch,
            ))
            .await
        {
            None => {}
            Some(Ok(())) => drain.done.extend(idx),
            Some(Err(e)) => self.on_listenbrainz_error(&e, "ListenBrainz submit", drain).await,
        }
    }

    /// A rejected session disconnects Last.fm and drops every flag still pending for it, which
    /// would otherwise retry against a credential just deleted; anything else defers.
    async fn on_lastfm_error<T>(&self, error: &LastfmError, what: &str, drain: &mut Drain<'_, T>) {
        match lastfm_reaction(error) {
            Reaction::Disconnect => {
                self.disconnect_lastfm().await;
                drain.drop_pending();
            }
            Reaction::Retry(delay) => drain.defer(what, error, delay),
        }
    }

    /// [`Self::on_lastfm_error`] for `ListenBrainz`.
    async fn on_listenbrainz_error<T>(
        &self,
        error: &ListenBrainzError,
        what: &str,
        drain: &mut Drain<'_, T>,
    ) {
        match listenbrainz_reaction(error) {
            Reaction::Disconnect => {
                self.disconnect_listenbrainz().await;
                drain.drop_pending();
            }
            Reaction::Retry(delay) => drain.defer(what, error, delay),
        }
    }

    /// Clear the submitted providers' flags by snapshot index (bounds-checked
    /// against a rare cap-drop shift — a double-submit is deduped by both
    /// services), `retain_pending`, and return the new queue when it changed (for
    /// the caller to persist off-thread). `select` picks the sub-queue —
    /// `items` for scrobbles, `loves` for loves — so both drains share this walk.
    ///
    /// `keep` guards each clear against the entry having changed under the lock
    /// while its POST was in flight: `submit_loves` passes a `loved`-match check
    /// so a favorite toggled the opposite way mid-submit isn't cleared (its newer
    /// state stays pending); `submit_scrobbles` passes `|_, _| true`.
    pub(super) fn collect_writeback<T: ProviderFlags>(
        &self,
        select: impl Fn(&mut ScrobbleQueue) -> &mut VecDeque<T>,
        clear_lastfm: &[usize],
        clear_lb: &[usize],
        keep: impl Fn(usize, &T) -> bool,
    ) -> Option<ScrobbleQueue> {
        let mut queue = self.queue.lock();
        let before = select(&mut queue).len();
        let mut cleared = false;
        for &i in clear_lastfm {
            if let Some(item) = select(&mut queue).get_mut(i)
                && keep(i, item)
            {
                item.set_lastfm_remaining(false);
                cleared = true;
            }
        }
        for &i in clear_lb {
            if let Some(item) = select(&mut queue).get_mut(i)
                && keep(i, item)
            {
                item.set_listenbrainz_remaining(false);
                cleared = true;
            }
        }
        queue.retain_pending();
        let changed = cleared || select(&mut queue).len() != before;
        changed.then(|| queue.clone())
    }

    /// Persist a post-drain queue snapshot off the async runtime (via
    /// [`Self::save_queue`]). Best-effort: a failed write is logged, and the item
    /// stays in the in-memory queue to be re-persisted on the next drain.
    async fn persist_queue(&self, snapshot: ScrobbleQueue) {
        if let Err(e) = self.save_queue(snapshot).await {
            log::warn!("Failed to persist scrobble queue after submit: {}", describe(&e));
        }
    }

    /// Drain the love queue: one POST per pending love (Last.fm
    /// `track.love`/`track.unlove`, `ListenBrainz` recording feedback), clearing
    /// the per-provider flag on success and auto-disconnecting on rejected auth —
    /// mirroring `submit_scrobbles`. Reads the shadow fresh so a disconnect from
    /// the scrobble pass in the same round is honored. Capped per round; the
    /// submitter loop re-drains while loves remain.
    async fn submit_loves(&self, stop: &CancellationToken) -> Option<Duration> {
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
        let mut lastfm = Drain::new(&snapshot, |it: &LoveItem| it.lastfm_remaining);
        if !lastfm_ready {
            lastfm.drop_pending();
        } else if let Some(creds) = lastfm_creds.as_ref()
            && let (Some(api_key), Some(secret)) =
                (lastfm::LASTFM_API_KEY, lastfm::LASTFM_SHARED_SECRET)
        {
            let session = LastfmSession { api_key, secret, session_key: &creds.session_key };
            self.love_on_lastfm(&mut lastfm, &client, &session, stop).await;
        } else {
            // Unreachable while `lastfm_reachable()` keeps its `is_configured()`
            // gate (see the matching arm in `submit_scrobbles`); drop rather than
            // spin if that ever changes.
            debug_assert!(
                false,
                "lastfm reachable but api keys/session absent — is_configured() invariant broke"
            );
            lastfm.drop_pending();
        }

        let mut lb = Drain::new(&snapshot, |it: &LoveItem| it.listenbrainz_remaining);
        if !lb_ready {
            lb.drop_pending();
        } else if let Some(creds) = lb_creds.as_ref() {
            self.love_on_listenbrainz(&mut lb, &client, &creds.token, stop).await;
        }

        // Only clear a love whose queued `loved` still matches what we submitted:
        // a favorite toggled the opposite way while the POST was in flight
        // coalesces a fresh `loved` into the same entry (`push_love`), and clearing
        // it by index would drop that newer state. Mismatches stay pending and go
        // out next round.
        if let Some(queue_snapshot) = self.collect_writeback(
            |q| &mut q.loves,
            &lastfm.done,
            &lb.done,
            |i, current: &LoveItem| snapshot.get(i).is_some_and(|s| s.loved == current.loved),
        ) {
            self.persist_queue(queue_snapshot).await;
        }
        merge_opt(lastfm.retry_after, lb.retry_after)
    }

    /// One `track.love`/`track.unlove` per pending love, up to the round's cap, stopping at the
    /// first failure.
    async fn love_on_lastfm(
        &self,
        drain: &mut Drain<'_, LoveItem>,
        client: &Client,
        session: &LastfmSession<'_>,
        stop: &CancellationToken,
    ) {
        let LastfmSession { api_key, secret, session_key } = *session;
        let snapshot = drain.snapshot;
        for (i, love) in snapshot.iter().enumerate() {
            if drain.done.len() >= SCROBBLE_BATCH_MAX {
                break;
            }
            if !love.lastfm_remaining {
                continue;
            }
            match stop
                .run_until_cancelled(lastfm::love(
                    client,
                    api_key,
                    secret,
                    session_key,
                    &love.track,
                    love.loved,
                ))
                .await
            {
                None => break,
                Some(Ok(())) => drain.done.push(i),
                Some(Err(e)) => {
                    self.on_lastfm_error(&e, "Last.fm love", drain).await;
                    break;
                }
            }
        }
    }

    /// One recording-feedback POST per pending love, as [`Self::love_on_lastfm`].
    async fn love_on_listenbrainz(
        &self,
        drain: &mut Drain<'_, LoveItem>,
        client: &Client,
        token: &str,
        stop: &CancellationToken,
    ) {
        let snapshot = drain.snapshot;
        for (i, love) in snapshot.iter().enumerate() {
            if drain.done.len() >= SCROBBLE_BATCH_MAX {
                break;
            }
            if !love.listenbrainz_remaining {
                continue;
            }
            let Some(mbid) = love.track.recording_mbid.as_deref() else {
                drain.done.push(i); // no MBID for LB to key on → nothing to do
                continue;
            };
            match stop
                .run_until_cancelled(listenbrainz::submit_feedback(
                    client,
                    &self.listenbrainz_base,
                    token,
                    mbid,
                    i8::from(love.loved),
                ))
                .await
            {
                None => break,
                Some(Ok(())) => drain.done.push(i),
                Some(Err(e)) => {
                    self.on_listenbrainz_error(&e, "ListenBrainz feedback", drain).await;
                    break;
                }
            }
        }
    }
}

/// The three strings every signed Last.fm call takes.
#[derive(Clone, Copy)]
struct LastfmSession<'a> {
    api_key: &'a str,
    secret: &'a str,
    session_key: &'a str,
}

/// One provider's pass over a drain round's snapshot: the indices it is done with, and the
/// longest wait it was asked for.
struct Drain<'a, T> {
    snapshot: &'a [T],
    pending: fn(&T) -> bool,
    done: Vec<usize>,
    retry_after: Option<Duration>,
}

impl<'a, T> Drain<'a, T> {
    fn new(snapshot: &'a [T], pending: fn(&T) -> bool) -> Self {
        Self { snapshot, pending, done: Vec::new(), retry_after: None }
    }

    /// Marks every entry still pending for this provider done, there being nowhere to send it.
    fn drop_pending(&mut self) {
        drop_flags(self.snapshot, self.pending, &mut self.done);
    }

    fn defer(&mut self, what: &str, cause: &dyn std::error::Error, delay: Duration) {
        log_deferral(what, cause, delay);
        self.retry_after = Some(merge_retry(self.retry_after, delay));
    }
}

/// What a failed provider call means for the queue. Both drains ask this of both providers, so
/// the policy is written once rather than four times, and is decidable without a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reaction {
    /// Auth was rejected: clear the credential and drop this provider's pending flags, which
    /// would otherwise retry forever against something we just deleted.
    Disconnect,
    /// Keep the entry queued. `Duration::ZERO` means the provider named no wait and the
    /// submitter's own backoff picks one.
    Retry(Duration),
}

/// Last.fm's retry policy. Only a rejected session disconnects: an unclassified code keeps its
/// queue slot deliberately, since one mis-read as permanent silently loses the listen.
fn lastfm_reaction(error: &LastfmError) -> Reaction {
    match error {
        LastfmError::InvalidSession => Reaction::Disconnect,
        _ => Reaction::Retry(Duration::ZERO),
    }
}

/// `ListenBrainz`'s retry policy. The one provider that names its own wait, via the 429's
/// `X-RateLimit-Reset-In`.
fn listenbrainz_reaction(error: &ListenBrainzError) -> Reaction {
    match error {
        ListenBrainzError::InvalidToken => Reaction::Disconnect,
        ListenBrainzError::RateLimited { reset_in_secs } => {
            Reaction::Retry(listenbrainz::rate_limit_backoff(*reset_in_secs))
        }
        _ => Reaction::Retry(Duration::ZERO),
    }
}

/// The one deferral line both drains log, carrying the wait only when the provider named one.
fn log_deferral(what: &str, cause: &dyn std::error::Error, delay: Duration) {
    if delay.is_zero() {
        log::info!("{what} deferred: {}", describe(cause));
    } else {
        log::info!("{what} deferred: {}; retrying in {}s", describe(cause), delay.as_secs());
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
    out.extend(snapshot.iter().enumerate().filter(|(_, it)| flag(it)).map(|(i, _)| i));
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
#[path = "tests/submit_tests.rs"]
mod tests;
