//! "Up Next" list subscriber + open-callback seeder. Both rebuilds are
//! gated on the view being visible.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, VecModel};

use super::source_change::apply_source_change;
use super::{NowPlayingState, UP_NEXT_N, current_track_id};
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::ui::queue_sheet::to_slint_queue_row;
use crate::ui::util::len_as_i32;
use melodia_app::state::AppState;
use melodia_engine::player::engine::state::QueueViewModel;
use melodia_ui::{AppWindow, Nav, NowPlaying, QueueRow};

/// Subscribe to `sinks.queue`. Closed, the subscriber only stashes the latest snapshot
/// into `NowPlayingState::latest_qvm`; open, it rebuilds the Up Next list when the
/// visible id slice actually changed, and resets `NowPlaying.slide-phase` on a real
/// current-track change.
pub(super) fn spawn_up_next_subscriber(
    ui: &AppWindow,
    state: &AppState,
    up_next_model: Rc<VecModel<QueueRow>>,
    np_state: Rc<NowPlayingState>,
) -> Result<(), slint::EventLoopError> {
    let weak = ui.as_weak();
    let mut rx = state.sinks.queue.subscribe();
    slint::spawn_local(Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = rx.borrow_and_update().clone();
            let Some(ui) = weak.upgrade() else { break };
            let Some(qvm) = snapshot else { continue };

            // Nothing that renders the model is on screen, so stash the snapshot for a
            // later open.
            if !np_state.open.get() && !np_state.mini_visible.get() {
                *np_state.latest_qvm.borrow_mut() = Some(qvm);
                continue;
            }

            // Rebuild only when the visible slice actually changed: a reorder or add
            // *below* the window leaves the rendered ids identical. A current-track
            // change always rebuilds, the base index shifting, and restarts the slide.
            let new_ids = upcoming_id_slice(&qvm);
            let current_id = current_track_id(&qvm);
            let track_changed = current_id != np_state.last_current_id.get();
            let ids_changed = *np_state.rendered_ids.borrow() != new_ids;
            // Before the rebuild overwrites them: needed only on a track change, to
            // look up the row that fell off the bottom for the outgoing overlay.
            let old_rendered_ids: Vec<i64> = if track_changed {
                np_state.rendered_ids.borrow().clone()
            } else {
                Vec::new()
            };
            if ids_changed || track_changed {
                let ids = rebuild_up_next(&ui, &up_next_model, &qvm);
                *np_state.rendered_ids.borrow_mut() = ids;
            }
            if track_changed {
                let kind = classify_step(np_state.last_queue_index.get(), &qvm);
                // The track that fell off the bottom of the *old* list becomes the
                // Backward step's outgoing overlay row — but only if it really fell
                // off, a short list that grew by one dropping nothing.
                let dropped_tail_id = old_rendered_ids
                    .last()
                    .copied()
                    .filter(|id| !np_state.rendered_ids.borrow().contains(id));
                let outgoing_row = outgoing_row(kind, &qvm, current_id, dropped_tail_id);

                np_state.last_current_id.set(current_id);
                np_state.last_queue_index.set(qvm.queue_index);
                // Direct writes, landing in the same render as the model swap above, so
                // there is no post-advance flash. Direction and outgoing row go
                // *before* the phase reset, so the timer's first tick already has the
                // right offset and content.
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
            *np_state.latest_qvm.borrow_mut() = Some(qvm);
        }
        log::debug!("ui::now_playing up-next subscriber stopped");
    }))?;
    Ok(())
}

/// Rebuild the Up Next list from `np_state.latest_qvm` (the snapshot the
/// subscriber stashes while no surface renders the model). No-op when no
/// snapshot has been stashed. Shared between:
///
/// - `wire_now_playing_open` — when the full-screen Now Playing view opens.
/// - `crate::ui::shell::mini_player::install` — via `NowPlayingState::kick_up_next`,
///   when the responsive miniplayer becomes visible.
///
/// Resets the slide bookkeeping so the next real track change picks the right direction
/// without replaying a phantom animation for changes that happened while closed.
pub(super) fn seed_from_stash(
    ui: &AppWindow,
    up_next_model: &Rc<VecModel<QueueRow>>,
    np_state: &NowPlayingState,
) {
    let latest_qvm = np_state.latest_qvm.borrow().clone();
    let Some(qvm) = latest_qvm else { return };
    let ids = rebuild_up_next(ui, up_next_model, &qvm);
    *np_state.rendered_ids.borrow_mut() = ids;
    np_state.last_current_id.set(current_track_id(&qvm));
    np_state.last_queue_index.set(qvm.queue_index);
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
            // Drop the decoded cover and blur buffers and hand the pages back. The
            // displayed track's stay alive, the `Player` global still referencing its
            // `Image`s, so a same-track reopen needs no decode. Off the UI thread —
            // `clear()` drops buffers and `trim()` walks arenas.
            let np_artwork = np_artwork.clone();
            state.runtime.spawn_blocking(move || {
                np_artwork.clear();
                melodia_platform::services::platform::allocator::trim();
            });
            return;
        }
        let Some(ui) = weak.upgrade() else { return };

        seed_from_stash(&ui, &up_next_model, &np_state);

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
                log::warn!("ui::now_playing open-seed task spawn_local: {e}");
            }
        }
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
    let len = len_as_i32(qvm.queue_tracks.len());
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

/// The "outgoing" row for the slide, if any: on `Forward` the just-promoted track, which
/// was at the top of the old list; on `Backward` the one that fell off the bottom, which
/// the caller only passes when it really fell off.
fn outgoing_row(
    kind: SlideKind,
    qvm: &QueueViewModel,
    current_id: Option<i64>,
    dropped_tail_id: Option<i64>,
) -> Option<QueueRow> {
    let id = match kind {
        SlideKind::Forward => current_id?,
        SlideKind::Backward => dropped_tail_id?,
        SlideKind::Other => return None,
    };
    let track = qvm.queue_tracks.iter().find(|t| t.id == id)?;
    Some(to_slint_queue_row(track.as_ref(), false))
}

/// Play-order indices of the next `UP_NEXT_N` tracks after the current one, plus the
/// base index — `queue_index + 1` clamped, since `queue_index` is `-1` with nothing
/// playing.
///
/// Under `RepeatMode::All` or `One` the queue is a cycle, so the slice wraps past the
/// end and stops just before the current track again; `Off` takes a plain forward slice.
pub(super) fn upcoming_indices(qvm: &QueueViewModel) -> (Vec<usize>, usize) {
    let len = qvm.queue_tracks.len();
    let base = usize::try_from(qvm.queue_index + 1).unwrap_or(0);
    let wrap = qvm.repeat_mode.wraps() && qvm.queue_index >= 0;
    let indices: Vec<usize> = if wrap && len > 0 {
        (0..(len - 1).min(UP_NEXT_N)).map(|n| (base + n) % len).collect()
    } else {
        (base..len.min(base + UP_NEXT_N)).collect()
    };
    (indices, base)
}

/// The track ids the list *would* render for `qvm` — the cheap pre-check the subscriber
/// skips an unchanged rebuild on.
fn upcoming_id_slice(qvm: &QueueViewModel) -> Vec<i64> {
    let (indices, _) = upcoming_indices(qvm);
    indices.iter().map(|&i| qvm.queue_tracks[i].id).collect()
}

/// Rebuild `up_next_model` and publish the base index and queue length, so a row click
/// can `Queue.skip-to` — with a modulo for the wrapped repeat-all case. Returns the
/// rendered id slice for the caller's `rendered_ids` shadow.
pub(super) fn rebuild_up_next(
    ui: &AppWindow,
    up_next_model: &Rc<VecModel<QueueRow>>,
    qvm: &QueueViewModel,
) -> Vec<i64> {
    let (indices, base) = upcoming_indices(qvm);
    let mut ids = Vec::with_capacity(indices.len());
    let mut upcoming = Vec::with_capacity(indices.len());
    for &i in &indices {
        let t = qvm.queue_tracks[i].as_ref();
        ids.push(t.id);
        upcoming.push(to_slint_queue_row(t, false));
    }

    crate::ui::model_diff::apply_rows_keyed(up_next_model, upcoming, |r| r.id);
    let np = ui.global::<NowPlaying>();
    np.set_base_index(i32::try_from(base).unwrap_or(i32::MAX));
    np.set_queue_length(len_as_i32(qvm.queue_tracks.len()));
    ids
}
