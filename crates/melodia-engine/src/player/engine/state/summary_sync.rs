//! Library edits reaching the track summaries the live state caches, so a retag or a star lands on
//! Now Playing and in the queue without a restart.

use std::collections::HashMap;
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

/// True when `current_track` or any `queue.tracks` entry has an id the
/// predicate accepts. Takes the state lock briefly and reads nothing else, so a
/// caller can cheaply decide whether a resync (and the DB refetch that feeds
/// it) is worth doing before paying for it — the membership gate
/// [`sync_track_summaries`] uses internally, hoisted so the fetch itself can be
/// skipped on the common "edited tracks aren't playing/queued" path.
pub fn any_tracked(state: &PlayerStateHandle, pred: impl Fn(i64) -> bool) -> bool {
    let g = lock_state(state);
    g.current_track().is_some_and(|t| pred(t.id)) || g.queue.tracks.iter().any(|t| pred(t.id))
}

/// Overwrite every queued / currently-playing [`TrackSummary`] whose id appears
/// in `fresh` with its fresh copy. Sibling of [`sync_current_track_if_in`], but
/// also walks `queue.tracks` — a tag edit changes exactly the title/artist/album
/// fields the Queue Sheet and Up Next render, not just the Now-Playing bar.
///
/// Pre-checks membership outside the emit lock so an edit touching nothing
/// queued/playing skips the publish entirely (the common case).
pub fn sync_track_summaries<S: std::hash::BuildHasher>(
    state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    fresh: &HashMap<i64, TrackSummary, S>,
) {
    if !any_tracked(state, |id| fresh.contains_key(&id)) {
        return;
    }

    with_state_emit(state, sinks, |s| {
        if let Some(track) = s.current_track_mut()
            && let Some(f) = fresh.get(&track.id)
        {
            *Arc::make_mut(track) = f.clone();
        }

        let mut queue_touched = false;
        for track in &mut s.queue.tracks {
            if let Some(f) = fresh.get(&track.id) {
                *Arc::make_mut(track) = f.clone();
                queue_touched = true;
            }
        }
        // A field-level `Arc::make_mut` doesn't advance the queue version on its
        // own, but `with_state_emit` only republishes the queue view-model when
        // the version changed — so bump it whenever a queued entry was patched,
        // or the Queue Sheet / Up Next would keep the stale summary.
        if queue_touched {
            s.queue.version += 1;
        }

        Vec::<PlayerAction>::new()
    });
}
