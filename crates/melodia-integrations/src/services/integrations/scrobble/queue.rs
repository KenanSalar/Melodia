//! Durable offline scrobble queue. A scrobble that can't be submitted right away
//! (offline, provider down, rate-limited) is persisted here with its real start
//! `timestamp` so it keeps the correct listen time whenever it finally drains.
//!
//! This is the pure serde model plus its cap / drop-oldest logic — no locking or
//! ownership of the file path. [`super::ScrobbleService`] is the managed handle
//! that guards it behind a mutex and flushes it to disk.

use std::collections::VecDeque;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::ScrobbleTrack;
use melodia_core::error::AppResult;

/// Upper bound on queued-but-unsent scrobbles. Generous enough to cover a long
/// offline stretch; beyond it the oldest listens are dropped (and logged) rather
/// than growing the file without bound.
const MAX_QUEUED: usize = 5_000;

/// One queued scrobble: the enriched track, the UNIX-seconds timestamp captured
/// when the track *started*, and a per-provider "still needs submitting" flag.
/// The submitter clears a flag on that provider's success; an item is done once
/// both flags are `false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedItem {
    pub track: ScrobbleTrack,
    pub timestamp: i64,
    pub lastfm_remaining: bool,
    pub listenbrainz_remaining: bool,
}

impl QueuedItem {
    /// Whether either provider still needs this listen submitted.
    pub fn is_pending(&self) -> bool {
        self.lastfm_remaining || self.listenbrainz_remaining
    }
}

/// One queued love/unlove: the track, its target loved state, and a per-provider
/// "still needs submitting" flag. Loves ride the same durable queue as scrobbles
/// so a favorite toggled offline still reaches the services on reconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoveItem {
    pub track: ScrobbleTrack,
    pub loved: bool,
    pub lastfm_remaining: bool,
    pub listenbrainz_remaining: bool,
}

impl LoveItem {
    /// Whether either provider still needs this love submitted.
    pub fn is_pending(&self) -> bool {
        self.lastfm_remaining || self.listenbrainz_remaining
    }
}

/// Set-only access to a queued entry's per-provider "still needs submitting"
/// flags, so one generic writeback can clear them on either sub-queue
/// (`items` or `loves`) without duplicating the walk per entry type.
pub(crate) trait ProviderFlags {
    fn set_lastfm_remaining(&mut self, remaining: bool);
    fn set_listenbrainz_remaining(&mut self, remaining: bool);
}

impl ProviderFlags for QueuedItem {
    fn set_lastfm_remaining(&mut self, remaining: bool) {
        self.lastfm_remaining = remaining;
    }
    fn set_listenbrainz_remaining(&mut self, remaining: bool) {
        self.listenbrainz_remaining = remaining;
    }
}

impl ProviderFlags for LoveItem {
    fn set_lastfm_remaining(&mut self, remaining: bool) {
        self.lastfm_remaining = remaining;
    }
    fn set_listenbrainz_remaining(&mut self, remaining: bool) {
        self.listenbrainz_remaining = remaining;
    }
}

/// FIFO of pending scrobbles and loves, oldest at the front. Serialized as-is to
/// `scrobble_queue.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrobbleQueue {
    pub items: VecDeque<QueuedItem>,
    /// Pending love/unlove submissions. `#[serde(default)]` keeps queue files
    /// written before love-sync (which carry no `loves` key) loadable.
    #[serde(default)]
    pub loves: VecDeque<LoveItem>,
}

impl ScrobbleQueue {
    /// Append a scrobble, dropping the oldest entries first if the cap is hit.
    pub fn push(&mut self, item: QueuedItem) {
        while self.items.len() >= MAX_QUEUED {
            self.items.pop_front();
            log::warn!("scrobble queue at cap ({MAX_QUEUED}); dropping oldest listen");
        }
        self.items.push_back(item);
    }

    /// Queue a love/unlove. If a still-pending love for the same track is already
    /// queued, fold this into it — take the newer `loved` state and OR in the
    /// provider flags — instead of appending, so a rapid heart→unheart submits
    /// once. Otherwise append, dropping the oldest past the cap like `push`.
    pub fn push_love(&mut self, item: LoveItem) {
        if let Some(existing) = self.loves.iter_mut().find(|queued| {
            queued.track.artist == item.track.artist && queued.track.track == item.track.track
        }) {
            existing.loved = item.loved;
            existing.lastfm_remaining |= item.lastfm_remaining;
            existing.listenbrainz_remaining |= item.listenbrainz_remaining;
            return;
        }
        while self.loves.len() >= MAX_QUEUED {
            self.loves.pop_front();
            log::warn!("love queue at cap ({MAX_QUEUED}); dropping oldest love");
        }
        self.loves.push_back(item);
    }

    /// Drop every scrobble and love both providers have already accepted, keeping
    /// only those with work left. Called by the submitter after a drain pass.
    pub fn retain_pending(&mut self) {
        self.items.retain(QueuedItem::is_pending);
        self.loves.retain(LoveItem::is_pending);
    }

    /// Read the queue file, defaulting to empty on a missing or unparseable file.
    pub fn load(path: &Path) -> AppResult<Self> {
        melodia_core::utils::atomic_file::load_json_or_default_sync(path)
    }

    /// Atomically persist the queue to `path`.
    pub fn save(&self, path: &Path) -> AppResult<()> {
        melodia_core::utils::atomic_file::write_json_sync(path, self)
    }
}

#[cfg(test)]
#[path = "tests/queue_tests.rs"]
mod tests;
