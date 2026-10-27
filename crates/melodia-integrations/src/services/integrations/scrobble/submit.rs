//! The submitter's drain: batched listens and per-item loves, gated per provider, disconnecting
//! on rejected auth and backing off on everything else.

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

/// Last.fm's cap on listens per POST, held for `ListenBrainz` too and for a round of loves.
const SCROBBLE_BATCH_MAX: usize = 50;

impl ScrobbleService {
    /// Best-effort; the caller drops the pending Last.fm flags itself.
    async fn disconnect_lastfm(&self) {
        log::warn!("Last.fm session invalid; disconnecting");
        if let Err(e) = self.set_lastfm_credentials(None).await {
            log::warn!("Failed to persist Last.fm disconnect: {}", describe(&e));
        }
    }

    async fn disconnect_listenbrainz(&self) {
        log::warn!("ListenBrainz token invalid; disconnecting");
        if let Err(e) = self.set_listenbrainz_credentials(None).await {
            log::warn!("Failed to persist ListenBrainz disconnect: {}", describe(&e));
        }
    }

    /// One round over listens and loves, `Some(delay)` where a provider deferred. Failures are
    /// logged, never toasted.
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

    async fn submit_scrobbles(&self, stop: &CancellationToken) -> Option<Duration> {
        let snapshot: Vec<QueuedItem> = {
            let queue = self.queue.lock();
            if queue.items.is_empty() {
                return None;
            }
            queue.items.iter().cloned().collect()
        };

        // Cloned out under one read, so no guard is held across a POST.
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
            lastfm.drop_pending();
        } else if let Some(creds) = lastfm_creds.as_ref()
            && let Some((api_key, secret)) = lastfm::keys()
        {
            let session = LastfmSession { api_key, secret, session_key: &creds.session_key };
            self.scrobble_to_lastfm(&mut lastfm, &client, &session, stop).await;
        } else {
            // Unreachable, `lastfm_ready` implying both keys and a session. Should that ever
            // break, dropping the flags keeps them from spinning the submitter at zero delay.
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
        let (batch, idx) = take_batch(drain.snapshot, drain.pending);
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
        let (batch, idx) = take_batch(drain.snapshot, drain.pending);
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

    async fn on_lastfm_error<T>(&self, error: &LastfmError, what: &str, drain: &mut Drain<'_, T>) {
        match lastfm_reaction(error) {
            Reaction::Disconnect => {
                self.disconnect_lastfm().await;
                drain.drop_pending();
            }
            Reaction::Retry(delay) => drain.defer(what, error, delay),
        }
    }

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

    /// Clears the submitted flags by snapshot index, returning the queue to persist when it
    /// changed. A cap drop mid-POST can shift the indices, hence the bounds check; the double
    /// submit that can follow is one both services dedupe. `keep` vetoes a clear for an entry
    /// that changed under the lock while its POST was in flight.
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

    /// Best-effort: a failed write leaves the queue in memory for the next drain to persist.
    async fn persist_queue(&self, snapshot: ScrobbleQueue) {
        if let Err(e) = self.save_queue(snapshot).await {
            log::warn!("Failed to persist scrobble queue after submit: {}", describe(&e));
        }
    }

    /// Reads the shadow afresh, so a disconnect in this round's scrobble pass holds here too.
    async fn submit_loves(&self, stop: &CancellationToken) -> Option<Duration> {
        let snapshot: Vec<LoveItem> = {
            let queue = self.queue.lock();
            if queue.loves.is_empty() {
                return None;
            }
            queue.loves.iter().cloned().collect()
        };

        // Reachability alone: the love toggles gate enqueuing, not the drain.
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
            && let Some((api_key, secret)) = lastfm::keys()
        {
            let session = LastfmSession { api_key, secret, session_key: &creds.session_key };
            self.love_on_lastfm(&mut lastfm, &client, &session, stop).await;
        } else {
            // As in `submit_scrobbles`.
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

        // A favorite toggled back mid-POST coalesced a newer `loved` into the same entry
        // (`push_love`), which a clear by index would drop; it goes out next round instead.
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

    async fn love_on_lastfm(
        &self,
        drain: &mut Drain<'_, LoveItem>,
        client: &Client,
        session: &LastfmSession<'_>,
        stop: &CancellationToken,
    ) {
        let LastfmSession { api_key, secret, session_key } = *session;
        let (snapshot, pending) = (drain.snapshot, drain.pending);
        for (i, love) in snapshot.iter().enumerate() {
            if drain.done.len() >= SCROBBLE_BATCH_MAX {
                break;
            }
            if !pending(love) {
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

    async fn love_on_listenbrainz(
        &self,
        drain: &mut Drain<'_, LoveItem>,
        client: &Client,
        token: &str,
        stop: &CancellationToken,
    ) {
        let (snapshot, pending) = (drain.snapshot, drain.pending);
        for (i, love) in snapshot.iter().enumerate() {
            if drain.done.len() >= SCROBBLE_BATCH_MAX {
                break;
            }
            if !pending(love) {
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

/// What a failed provider call means for the queue, decided without a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reaction {
    /// Auth was rejected: clear the credential and drop this provider's pending flags, which
    /// would otherwise retry against a credential just deleted.
    Disconnect,
    /// Keep the entry queued. `Duration::ZERO` means the provider named no wait and the
    /// submitter's own backoff picks one.
    Retry(Duration),
}

/// Only a rejected session disconnects. Any other code keeps its slot, a permanent one (10, an
/// invalid key; 26, a suspended one) until the cap evicts it, since a code mis-read as permanent
/// silently loses the listen.
fn lastfm_reaction(error: &LastfmError) -> Reaction {
    match error {
        LastfmError::InvalidSession => Reaction::Disconnect,
        _ => Reaction::Retry(Duration::ZERO),
    }
}

/// The one provider that names its own wait, in the 429's `X-RateLimit-Reset-In`.
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

/// The loop honors the longest wait any provider asked for.
fn merge_retry(current: Option<Duration>, requested: Duration) -> Duration {
    current.map_or(requested, |c| c.max(requested))
}

fn merge_opt(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (some, None) | (None, some) => some,
    }
}

fn drop_flags<T>(snapshot: &[T], flag: impl Fn(&T) -> bool, out: &mut Vec<usize>) {
    out.extend(snapshot.iter().enumerate().filter(|(_, it)| flag(it)).map(|(i, _)| i));
}

/// Up to [`SCROBBLE_BATCH_MAX`] pending listens, with their snapshot indices.
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
