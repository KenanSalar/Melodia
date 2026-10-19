//! Library edits reaching the track records the live state holds, so a retag or a star lands on
//! Now Playing and on the next track without a restart.
//!
//! Two paths: a list-row edit patches the playing track at once, and the app's window task
//! re-reads every record the state holds after each queue move and library change.

use std::sync::Arc;

use super::{PlayerAction, PlayerStateHandle, lock_state, with_state_emit};
use crate::player::engine::event_sink::PlayerSinks;
use melodia_core::entities::track::TrackSummary;

/// If `current_track` is one of `ids`, apply `apply` to its cached
/// [`TrackSummary`] and emit so the Now-Playing surfaces reflect a per-track
/// field edited from a list row (favorite heart, rating stars). Skips the emit
/// entirely when the playing track isn't in the set (the common case) to avoid
/// a spurious view-model publish.
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
        // Re-check the id under the emit lock — the track may have advanced
        // between the pre-check and here.
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

/// The ids whose records the window task should fetch for the queue as it stands.
///
/// `All` adds the playing track where the window doesn't name it, a queue cleared under a playing
/// track leaving it on the deck with no slot.
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

/// Seat what the window task fetched: the records the window still wants, against the queue as it
/// stands now rather than when the fetch was asked for, and the playing track's own where it
/// changed.
///
/// The window is no view model's, so filling it publishes nothing. Only a changed playing track
/// emits, which is how a retag reaches Now Playing.
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
