//! Library edits reaching the track records the live state holds: a list-row edit patches the
//! playing track at once, and the app's window task re-reads every record after each queue move
//! and library change.

use std::sync::Arc;

use super::{PlayerAction, PlayerStateHandle, lock_state, with_state_emit};
use crate::player::engine::event_sink::PlayerSinks;
use melodia_core::entities::track::TrackSummary;

/// Applies a list-row edit (a heart, the stars) to the playing track where it is one of `ids`,
/// skipping the emit in the common case where it isn't.
pub fn sync_current_track_if_in(
    state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    ids: &[i64],
    apply: impl FnOnce(&mut TrackSummary),
) {
    let affects_current = {
        let g = lock_state(state);
        g.current_track().is_some_and(|t| ids.contains(&t.id))
    };
    if !affects_current {
        return;
    }
    with_state_emit(state, sinks, |s| {
        // Re-checked under the emit lock: the track may have advanced since.
        if let Some(track) = s.current_track_mut()
            && ids.contains(&track.id)
        {
            apply(Arc::make_mut(track));
        }
        Vec::<PlayerAction>::new()
    });
}

/// Which records [`window_wanted`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFill {
    /// The ones the window lacks, after the queue moved.
    Missing,
    /// Every one the state holds, after a library change that may have rewritten any of them.
    All,
}

/// `All` adds the playing track where the window doesn't name it, as a queue cleared under it
/// leaves it.
pub fn window_wanted(state: &PlayerStateHandle, fill: WindowFill) -> Vec<i64> {
    let s = lock_state(state);
    let wanted = s.queue.window_ids();
    match fill {
        WindowFill::Missing => s.window.missing(&wanted),
        WindowFill::All => {
            let mut ids = wanted;
            if let Some(playing) = s.current_track().map(|track| track.id)
                && !ids.contains(&playing)
            {
                ids.push(playing);
            }
            ids
        }
    }
}

/// Seats what the window task fetched against the queue as it stands now. Only a changed playing
/// track emits, which is how a retag reaches Now Playing.
pub fn seat_window_records(
    state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    fresh: Vec<TrackSummary>,
) {
    let fresh: Vec<Arc<TrackSummary>> = fresh.into_iter().map(Arc::new).collect();
    let playing_moved = {
        let mut s = lock_state(state);
        let wanted = s.queue.window_ids();
        s.window.seat(&wanted, fresh.iter().cloned());
        s.current_track().and_then(|track| {
            fresh
                .iter()
                .find(|&record| record.id == track.id && differs_beyond_position(record, track))
                .cloned()
        })
    };
    let Some(record) = playing_moved else {
        return;
    };
    with_state_emit(state, sinks, |s| {
        // Re-checked under the emit lock: the track may have changed since the read above.
        if let Some(track) = s.current_track_mut()
            && track.id == record.id
        {
            *track = record;
        }
        Vec::<PlayerAction>::new()
    });
}

/// Whether `fresh` reads differently from `held` in anything but `last_position`, which the
/// periodic save rewrites as the track plays and so says nothing about an edit.
fn differs_beyond_position(fresh: &TrackSummary, held: &TrackSummary) -> bool {
    TrackSummary { last_position: held.last_position, ..fresh.clone() } != *held
}
