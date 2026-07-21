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
use crate::error::AppResult;
use crate::services;

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

/// FIFO of pending scrobbles, oldest at the front. Serialized as-is to
/// `scrobble_queue.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrobbleQueue {
    pub items: VecDeque<QueuedItem>,
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

    /// Drop every item both providers have already accepted, keeping only those
    /// with work left. Called by the submitter after a drain pass.
    pub fn retain_pending(&mut self) {
        self.items.retain(QueuedItem::is_pending);
    }

    /// Read the queue file, defaulting to empty on a missing or unparseable file.
    pub fn load(path: &Path) -> AppResult<Self> {
        services::load_json_or_default_sync(path)
    }

    /// Atomically persist the queue to `path`.
    pub fn save(&self, path: &Path) -> AppResult<()> {
        services::write_json_atomic_sync(path, self)
    }
}

#[cfg(test)]
#[path = "tests/queue_tests.rs"]
mod tests;
