//! The durable offline queue: a listen that can't go out now keeps its real start time until it
//! drains. The serde model and its cap; [`super::ScrobbleService`] holds the lock and the file.

use std::collections::VecDeque;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::ScrobbleTrack;
use melodia_core::error::AppResult;

/// Past this the oldest entries drop, logged, rather than the file growing without bound.
const MAX_QUEUED: usize = 5_000;

/// `timestamp` is when the track *started*, in UNIX seconds. Each provider's flag clears on its
/// own success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedItem {
    pub track: ScrobbleTrack,
    pub timestamp: i64,
    pub lastfm_remaining: bool,
    pub listenbrainz_remaining: bool,
}

impl QueuedItem {
    pub fn is_pending(&self) -> bool {
        self.lastfm_remaining || self.listenbrainz_remaining
    }
}

/// Rides the same durable queue, so a favorite toggled offline still reaches the services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoveItem {
    pub track: ScrobbleTrack,
    pub loved: bool,
    pub lastfm_remaining: bool,
    pub listenbrainz_remaining: bool,
}

impl LoveItem {
    pub fn is_pending(&self) -> bool {
        self.lastfm_remaining || self.listenbrainz_remaining
    }
}

/// Lets one writeback clear the flags on either sub-queue.
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

/// Oldest at the front, serialized as-is to `scrobble_queue.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrobbleQueue {
    pub items: VecDeque<QueuedItem>,
    /// Defaulted, since a queue written before love sync has no `loves` key.
    #[serde(default)]
    pub loves: VecDeque<LoveItem>,
}

impl ScrobbleQueue {
    pub fn push(&mut self, item: QueuedItem) {
        while self.items.len() >= MAX_QUEUED {
            self.items.pop_front();
            log::warn!("scrobble queue at cap ({MAX_QUEUED}); dropping oldest listen");
        }
        self.items.push_back(item);
    }

    /// Folds into a still-pending love for the same track, the newer `loved` winning, so a quick
    /// heart and unheart submits once.
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

    pub fn retain_pending(&mut self) {
        self.items.retain(QueuedItem::is_pending);
        self.loves.retain(LoveItem::is_pending);
    }

    pub fn load(path: &Path) -> AppResult<Self> {
        melodia_core::utils::atomic_file::load_json_or_default_sync(path)
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        melodia_core::utils::atomic_file::write_json_sync(path, self)
    }
}

#[cfg(test)]
#[path = "tests/queue_tests.rs"]
mod tests;
