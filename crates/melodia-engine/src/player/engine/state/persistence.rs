//! Both directions of `queue.json`: the snapshot a save writes, and the restore that seats it back
//! at boot.

use std::sync::Arc;

use super::{PlayerAction, PlayerState};
use crate::player::engine::queue::QueueState;
use crate::player::engine::types::{
    PersistedPlayback, PlaybackSource, PlaybackStatus, RadioNowPlaying,
};
use melodia_core::entities::track::TrackSummary;

impl PlayerState {
    /// Takes the whole state, a station riding beside the queue rather than in it.
    pub fn to_persisted(&self) -> PersistedPlayback {
        PersistedPlayback {
            queue: self.queue.to_persistable(),
            // A station with no row cannot be looked back up, so there is nothing to write down.
            station_id: self.station().map(|s| s.station_id).filter(|id| *id != 0),
        }
    }
}

/// Seats a restored queue and its window's records, the current track where it was left.
/// `library::queue::restore_persisted_playback` has already dropped the ids whose rows are gone.
pub fn restore_queue(state: &mut PlayerState, queue: QueueState, records: Vec<Arc<TrackSummary>>) {
    state.queue = queue;
    state.window.seat(&state.queue.window_ids(), records);

    if let Some(track) = state.queue.current_id().and_then(|id| state.record(id)) {
        state.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
        state.position_ms = u64::try_from(track.last_position.max(0)).unwrap_or(0);
        state.source = Some(PlaybackSource::Track(track));
    }
}

/// Puts the last session's station back over the queue [`restore_queue`] seated, `Paused`: the
/// one status holding a station with no connection, which `player_play` reopens from. The caller
/// owes an `emit_and_execute` for the speed pin, boot restoring a rate from `settings.json`.
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
