//! "Up Next" list subscriber + open-callback seeder. Both rebuilds are
//! gated on the view being visible.
//!
//! The queue publishes ids, so the list fetches the rows of the slice it shows by id. It shows
//! them, and plays a track change's slide, in the one tick the fetch lands in, rather than painting
//! placeholders first: twenty rows are one short read.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Model, VecModel, Weak};

use super::source_change::apply_source_change;
use super::{NowPlayingState, UP_NEXT_N, current_track_id};
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::ui::paged_rows::PagedRow;
use crate::ui::paged_rows::queue_rows;
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_engine::player::engine::state::QueueViewModel;
use melodia_ui::{AppWindow, Drag, DragOrigin, Nav, NowPlaying, QueueRow};

/// Follow `sinks.queue` and `library_changed`. Closed, the subscriber only stashes the latest
/// snapshot into `NowPlayingState::latest_qvm`; open, it rebuilds the Up Next list when the visible
/// id slice or the current track moved, and reads the rows again on a library tick, a retag
/// changing what a row says without moving the queue.
pub(super) fn spawn_up_next_subscriber(
    ui: &AppWindow,
    state: &AppState,
    up_next_model: Rc<VecModel<QueueRow>>,
    np_state: Rc<NowPlayingState>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let state = state.clone();
    let mut queue_rx = state.sinks.queue.subscribe();
    let mut library_rx = state.library_changed.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            let library_tick = tokio::select! {
                changed = queue_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    false
                }
                changed = library_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    true
                }
            };
            if !library_tick {
                let snapshot = queue_rx.borrow_and_update().clone();
                if snapshot.is_some() {
                    *np_state.latest_qvm.borrow_mut() = snapshot;
                }
            }

            // Nothing that renders the model is on screen; the stash is what a later open seeds
            // from.
            if !np_state.open.get() && !np_state.mini_visible.get() {
                continue;
            }
            if library_tick {
                seed_from_stash(&weak, &state, &up_next_model, &np_state).await;
                continue;
            }
            let latest = np_state.latest_qvm.borrow().clone();
            if let Some(qvm) = latest {
                follow(&weak, &state, &up_next_model, &np_state, &qvm).await;
            }
        }
        log::debug!("ui::now_playing up-next subscriber stopped");
    }))?;
    Ok(())
}

/// Bring the list up to `qvm`. A queue edit that leaves the visible slice and the current track
/// alone (a reorder *below* the window) moves nothing; anything else shows the slice's rows, and a
/// current-track change also restarts the slide.
async fn follow(
    weak: &Weak<AppWindow>,
    state: &AppState,
    up_next_model: &VecModel<QueueRow>,
    np_state: &NowPlayingState,
    qvm: &QueueViewModel,
) {
    let new_ids = upcoming_id_slice(qvm);
    let current_id = current_track_id(qvm);
    let track_changed = current_id != np_state.last_current_id.get();
    let ids_changed = *np_state.rendered_ids.borrow() != new_ids;
    if !ids_changed && !track_changed {
        return;
    }

    // Read off the rows on show before the fetch replaces them: on a forward step the promoted
    // track was the top row, and on a backward one the row falling off the bottom was the last,
    // but only where it really fell off, a short list that grew by one dropping nothing.
    let outgoing = track_changed.then(|| {
        let kind = classify_step(np_state.last_queue_index.get(), qvm);
        let shown: Vec<QueueRow> = up_next_model.iter().collect();
        let dropped_tail_id =
            np_state.rendered_ids.borrow().last().copied().filter(|id| !new_ids.contains(id));
        (kind, outgoing_row(kind, &shown, current_id, dropped_tail_id))
    });

    let Some(fetched) = fetch_slice(state, np_state, &new_ids).await else {
        return;
    };
    let Some(ui) = weak.upgrade() else { return };
    show_slice(&ui, up_next_model, np_state, qvm, new_ids, fetched);

    let Some((kind, outgoing_row)) = outgoing else {
        return;
    };
    np_state.last_current_id.set(current_id);
    np_state.last_queue_index.set(qvm.queue_index);
    // Direct writes, landing in the same render as the model swap above, so there is no
    // post-advance flash. Direction and outgoing row go *before* the phase reset, so the timer's
    // first tick already has the right offset and content.
    let np = ui.global::<NowPlaying>();
    if let Some(row) = outgoing_row {
        np.set_outgoing_row(row);
        np.set_outgoing_visible(true);
    } else {
        np.set_outgoing_visible(false);
    }
    np.set_slide_direction(kind.direction());
    np.set_slide_phase(0);
}

/// Rebuild the Up Next list from `np_state.latest_qvm` (the snapshot the
/// subscriber stashes while no surface renders the model). No-op when no
/// snapshot has been stashed. Shared between:
///
/// - `wire_now_playing_open` — when the full-screen Now Playing view opens.
/// - `crate::ui::shell::mini_player::install` — via `NowPlayingState::kick_up_next`,
///   when the responsive miniplayer becomes visible.
/// - the subscriber, on a library tick, the slice's ids standing and its rows maybe not.
///
/// Resets the slide bookkeeping so the next real track change picks the right direction
/// without replaying a phantom animation for changes that happened while closed.
pub(super) async fn seed_from_stash(
    weak: &Weak<AppWindow>,
    state: &AppState,
    up_next_model: &VecModel<QueueRow>,
    np_state: &NowPlayingState,
) {
    let latest = np_state.latest_qvm.borrow().clone();
    let Some(qvm) = latest else { return };
    let ids = upcoming_id_slice(&qvm);
    let Some(fetched) = fetch_slice(state, np_state, &ids).await else {
        return;
    };
    let Some(ui) = weak.upgrade() else { return };
    show_slice(&ui, up_next_model, np_state, &qvm, ids, fetched);
    np_state.last_current_id.set(current_track_id(&qvm));
    np_state.last_queue_index.set(qvm.queue_index);
}

/// [`seed_from_stash`] off a synchronous caller, on the event loop.
pub(super) fn spawn_seed(
    weak: Weak<AppWindow>,
    state: AppState,
    up_next_model: Rc<VecModel<QueueRow>>,
    np_state: Rc<NowPlayingState>,
) {
    let spawned = slint::spawn_local(Compat::new(async move {
        seed_from_stash(&weak, &state, &up_next_model, &np_state).await;
    }));
    if let Err(e) = spawned {
        log::warn!("ui::now_playing up-next seed not spawned: {}", describe(&e));
    }
}

/// Wire `Nav.now-playing-open-changed`, mirrored from `app-window.slint`. Updates the
/// shared `open` flag and, on open, seeds what the two subscribers skipped while closed.
pub(super) fn wire_now_playing_open(
    ui: &AppWindow,
    state: &AppState,
    np_artwork: Arc<NowPlayingArtwork>,
    up_next_model: Rc<VecModel<QueueRow>>,
    np_state: Rc<NowPlayingState>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    ui.global::<Nav>().on_now_playing_open_changed(move |is_open| {
        np_state.open.set(is_open);
        if !is_open {
            // **Each half asks whether anything still renders what it would hand back, and the
            // two answers differ.** Entering the miniplayer force-closes this view, so both run on
            // every entry, and releasing there costs the miniplayer the sheet and the cover it
            // is about to ask for again.
            if !np_state.renders_panel()
                && let Some(ui) = weak.upgrade()
            {
                super::release_lyrics(&ui, &state, &np_state);
            }

            // A same-track reopen needs no decode, the displayed track's images outliving the
            // release.
            if !np_state.renders_artwork() {
                np_state.release_artwork();
            }
            return;
        }

        spawn_seed(weak.clone(), state.clone(), up_next_model.clone(), np_state.clone());

        // Artwork and chips, but only when the source differs from what is already in
        // the `Player` global — a close and re-open with no change between needs no
        // decode. `animate = false`: the cover should already be there when the view
        // appears, not cross-fade in.
        let current_source = np_state.current_source.borrow().clone();
        let current_key = current_source.as_ref().map(|s| s.key.clone());
        if current_key != *np_state.applied_source.borrow() {
            let weak = weak.clone();
            let state = state.clone();
            let np_artwork = np_artwork.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                apply_source_change(&weak, &state, &np_artwork, &np_state, current_source, false)
                    .await;
            }));
            if let Err(e) = res {
                log::warn!("ui::now_playing open-seed task spawn_local: {}", describe(&e));
            }
        }

        // **Outside that guard, and that is the whole point of it being here.** The close above
        // hands the sheet back, so a re-open on the same track needs one again — but the artwork
        // is still applied, so the branch that would have fetched it returns early. It dedupes on
        // its own claim, so on the open that *did* re-decode this costs nothing.
        np_state.lyrics.kick();
    });
}

/// What kind of step the current-track change was, driving both the slide direction and
/// which row, if any, renders as the transient "outgoing" overlay.
#[derive(Clone, Copy)]
enum SlideKind {
    /// A forward step, wrap from last to first included. The just-promoted track slides
    /// off the top with the list.
    Forward,
    /// A backward step, wrap included. The row that fell off the visible bottom slides
    /// off the bottom.
    Backward,
    /// A skip-to or a queue rebuild: animates forward, with no outgoing overlay.
    Other,
}

impl SlideKind {
    fn direction(self) -> i32 {
        match self {
            Self::Backward => -1,
            _ => 1,
        }
    }
}

/// Classify the transition from `old_idx`, recognising single-step moves *with*
/// wrap-around so repeat-all and repeat-one navigation animates the right way.
fn classify_step(old_idx: i32, qvm: &QueueViewModel) -> SlideKind {
    let new_idx = qvm.queue_index;
    let len = len_as_i32(qvm.ids.len());
    if len <= 0 || old_idx < 0 || new_idx < 0 {
        return SlideKind::Other;
    }
    if new_idx == (old_idx + 1) % len {
        SlideKind::Forward
    } else if new_idx == (old_idx - 1).rem_euclid(len) {
        SlideKind::Backward
    } else {
        SlideKind::Other
    }
}

/// The "outgoing" row for the slide, if any, out of the rows the list was showing: on `Forward`
/// the just-promoted track, which was at their top; on `Backward` the one that fell off the
/// bottom, which the caller only passes when it really fell off.
fn outgoing_row(
    kind: SlideKind,
    shown: &[QueueRow],
    current_id: Option<i64>,
    dropped_tail_id: Option<i64>,
) -> Option<QueueRow> {
    let id = match kind {
        SlideKind::Forward => current_id?,
        SlideKind::Backward => dropped_tail_id?,
        SlideKind::Other => return None,
    };
    shown.iter().find(|row| i64::from(row.id) == id).cloned()
}

/// Play-order indices of the next `UP_NEXT_N` tracks after the current one, plus the
/// base index — `queue_index + 1` clamped, since `queue_index` is `-1` with nothing
/// playing.
///
/// Under `RepeatMode::All` or `One` the queue is a cycle, so the slice wraps past the
/// end and stops just before the current track again; `Off` takes a plain forward slice.
pub(super) fn upcoming_indices(qvm: &QueueViewModel) -> (Vec<usize>, usize) {
    let len = qvm.ids.len();
    let base = usize::try_from(qvm.queue_index + 1).unwrap_or(0);
    let wrap = qvm.repeat_mode.wraps() && qvm.queue_index >= 0;
    let indices: Vec<usize> = if wrap && len > 0 {
        (0..(len - 1).min(UP_NEXT_N)).map(|n| (base + n) % len).collect()
    } else {
        (base..len.min(base + UP_NEXT_N)).collect()
    };
    (indices, base)
}

/// The track ids the list renders for `qvm`, in its order.
fn upcoming_id_slice(qvm: &QueueViewModel) -> Vec<i64> {
    let (indices, _) = upcoming_indices(qvm);
    indices.iter().map(|&i| qvm.ids[i]).collect()
}

/// The rows behind `ids`, fetched on the runtime so the connection is back in the pool however
/// busy the event loop is drawing. `None` where the fetch failed, or where a later one was asked
/// for while it ran, whose answer is the one to show.
async fn fetch_slice(
    state: &AppState,
    np_state: &NowPlayingState,
    ids: &[i64],
) -> Option<Vec<(i64, QueueRow)>> {
    let ticket = np_state.up_next_ticket.get().wrapping_add(1);
    np_state.up_next_ticket.set(ticket);
    let db = state.db.clone();
    let asked = ids.to_vec();
    let fetched = state
        .runtime
        .spawn(async move {
            library::keyset::rows_by_ids(&db, &asked).await.map(|rows| queue_rows::convert(&rows))
        })
        .await;
    let rows = match fetched {
        Ok(Ok(rows)) => rows,
        Ok(Err(e)) => {
            log::warn!("ui::now_playing up-next fetch failed: {}", describe(&e));
            return None;
        }
        Err(e) => {
            log::warn!("ui::now_playing up-next fetch task failed: {}", describe(&e));
            return None;
        }
    };
    (np_state.up_next_ticket.get() == ticket).then_some(rows)
}

/// Show `ids` in `up_next_model` from the `fetched` rows, and publish the base index and queue
/// length, so a row click can `Queue.skip-to` with a modulo for the wrapped repeat-all case.
///
/// A row the fetch didn't return, its track deleted since the queue was read, shows as a
/// placeholder rather than closing up, which would point every click below it one track off.
///
/// A slice that moved revokes a drag still in flight: it goes on, the list's source surviving its
/// rows, but the index it lifted no longer names the same track. A library tick reseeds the same
/// slice, which leaves it be.
fn show_slice(
    ui: &AppWindow,
    up_next_model: &VecModel<QueueRow>,
    np_state: &NowPlayingState,
    qvm: &QueueViewModel,
    ids: Vec<i64>,
    fetched: Vec<(i64, QueueRow)>,
) {
    let (_, base) = upcoming_indices(qvm);
    let base = i32::try_from(base).unwrap_or(i32::MAX);
    let np = ui.global::<NowPlaying>();
    if *np_state.rendered_ids.borrow() != ids || np.get_base_index() != base {
        ui.global::<Drag>().invoke_revoke(DragOrigin::UpNext);
    }

    let by_id: HashMap<i64, QueueRow> = fetched.into_iter().collect();
    let rows: Vec<QueueRow> = ids
        .iter()
        .map(|&id| by_id.get(&id).cloned().unwrap_or_else(|| QueueRow::placeholder(id)))
        .collect();
    crate::ui::model_diff::apply_rows_keyed(up_next_model, rows, |r| r.id);

    np.set_base_index(base);
    np.set_queue_length(len_as_i32(qvm.ids.len()));
    *np_state.rendered_ids.borrow_mut() = ids;
}

/// Maps a move between two Up Next rows onto play order, the inverse of [`upcoming_indices`].
///
/// `None` for a move across the repeat-all wrap, which would carry the track from one end of the
/// queue to the other, past the one playing.
fn play_order_move(
    base: usize,
    len: usize,
    from_row: usize,
    to_row: usize,
) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let past_wrap = |row: usize| base + row >= len;
    if past_wrap(from_row) != past_wrap(to_row) {
        return None;
    }
    Some(((base + from_row) % len, (base + to_row) % len))
}

/// Wire `NowPlaying.reorder`. The row moves on show first, as the queue panel's does, so the
/// publish that follows finds the slice already rendered and repaints nothing.
pub(super) fn wire_reorder(
    ui: &AppWindow,
    state: &AppState,
    up_next_model: Rc<VecModel<QueueRow>>,
    np_state: Rc<NowPlayingState>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    ui.global::<NowPlaying>().on_reorder(move |from_row, to_row| {
        let Some(ui) = weak.upgrade() else { return };
        let np = ui.global::<NowPlaying>();
        let (Ok(base), Ok(len), Ok(from_row), Ok(to_row)) = (
            usize::try_from(np.get_base_index()),
            usize::try_from(np.get_queue_length()),
            usize::try_from(from_row),
            usize::try_from(to_row),
        ) else {
            return;
        };
        let Some((from, to)) = play_order_move(base, len, from_row, to_row) else {
            return;
        };
        move_shown_row(&up_next_model, &np_state, from_row, to_row);
        let s = state.clone();
        state.runtime.clone().spawn(async move {
            if let Err(e) = library::queue::queue_move(&s, from, to) {
                log::warn!("queue_move({from},{to}): {}", describe(&e));
            }
        });
    });
}

/// Moves a shown row, and the id it stands for, from `from` to `to`.
fn move_shown_row(model: &VecModel<QueueRow>, np_state: &NowPlayingState, from: usize, to: usize) {
    let mut ids = np_state.rendered_ids.borrow_mut();
    let shown = ids.len();
    if from >= shown || to >= shown || model.row_count() != shown {
        return;
    }
    let row = model.remove(from);
    model.insert(to, row);
    let id = ids.remove(from);
    ids.insert(to, id);
}

#[cfg(test)]
#[path = "tests/up_next_tests.rs"]
mod tests;
