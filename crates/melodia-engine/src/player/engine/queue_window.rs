//! The records behind the queue slots a transition can move onto. A transition runs under the
//! state lock, where nothing waits on the database, so the app's window task fetches them ahead
//! against [`QueueState::window_ids`](super::queue::QueueState::window_ids).

use std::sync::Arc;

use melodia_core::entities::track::TrackSummary;

/// The records of the tracks around the current slot, each once by id: a track queued twice is
/// one record.
#[derive(Debug, Clone, Default)]
pub struct TrackWindow {
    records: Vec<Arc<TrackSummary>>,
}

impl TrackWindow {
    pub fn get(&self, id: i64) -> Option<&Arc<TrackSummary>> {
        self.records.iter().find(|record| record.id == id)
    }

    pub fn missing(&self, wanted: &[i64]) -> Vec<i64> {
        wanted.iter().copied().filter(|&id| self.get(id).is_none()).collect()
    }

    /// Take `fresh` over whatever it held for the same ids, then keep only what `wanted` names.
    pub fn seat(&mut self, wanted: &[i64], fresh: impl IntoIterator<Item = Arc<TrackSummary>>) {
        for record in fresh {
            match self.records.iter_mut().find(|held| held.id == record.id) {
                Some(held) => *held = record,
                None => self.records.push(record),
            }
        }
        self.records.retain(|record| wanted.contains(&record.id));
    }
}
