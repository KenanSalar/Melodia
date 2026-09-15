//! Both directions of `queue.json`: the snapshot a save writes, and the restore that seats it back
//! at boot.

use std::sync::Arc;

use super::{PlayerAction, PlayerState};
use crate::player::engine::queue::current_index_from_i32;
use crate::player::engine::types::{
    PersistableQueue, PersistedPlayback, PlaybackSource, PlaybackStatus, RadioNowPlaying,
};
use melodia_core::entities::track::TrackSummary;

impl PlayerState {
    /// The snapshot `queue.json` holds, for the two save sites to write.
    ///
    /// Takes the whole state rather than the queue alone because a station rides beside the
    /// queue rather than in it, and both come back together at boot.
    pub fn to_persisted(&self) -> PersistedPlayback {
        PersistedPlayback {
            queue: self.queue.to_persistable(),
            // A station with no row cannot be looked back up, so there is nothing to write down.
            station_id: self.station().map(|s| s.station_id).filter(|id| *id != 0),
        }
    }
}

/// Restore queue from persisted data. Called at startup via
/// `library::queue::restore_persisted_playback`.
///
/// `shuffle_enabled` and `repeat_mode` are user preferences and live in
/// `settings.json`, not `queue.json` — the caller is responsible for
/// hydrating them. Note that `original_order` is not persisted, so a
/// caller restoring a non-empty queue should also force shuffle off.
pub fn restore_queue(
    state: &mut PlayerState,
    tracks: Vec<Arc<TrackSummary>>,
    persistable: &PersistableQueue,
) {
    let len = tracks.len();
    state.queue.tracks = tracks;
    state.queue.play_order = (0..len).collect();
    state.queue.original_order = (0..len).collect();
    state.queue.current_index = current_index_from_i32(persistable.current_index);

    if let Some(track) = state.queue.get_current().cloned() {
        state.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
        state.position_ms = u64::try_from(track.last_position.max(0)).unwrap_or(0);
        state.source = Some(PlaybackSource::Track(track));
    }
}

/// Put the station the last session was tuned to back on the deck, over the queue
/// [`restore_queue`] has already restored. Called from the same startup path.
///
/// `Paused` because that is already the one status holding a station with no connection —
/// pausing one drops its socket, so a restart is the same shape, and
/// `library::playback::player_play` re-opens from it. Seating the station is what evicts whatever
/// [`restore_queue`] just put on the deck, the two being one field.
///
/// Returns actions, so the caller owes an `emit_and_execute`: the speed pin is a backend write,
/// and boot is the one place a station can arrive over a rate `settings.json` restored.
pub fn restore_station(
    state: &mut PlayerState,
    station: Arc<RadioNowPlaying>,
) -> Vec<PlayerAction> {
    state.source = Some(PlaybackSource::Station(station));
    state.status = PlaybackStatus::Paused;
    state.duration_ms = 0;
    state.position_ms = 0;
    state.pin_speed_for_station().into_iter().collect()
}
