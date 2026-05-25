//! "Up Next" list subscriber + open-callback seeder. Both rebuilds are
//! gated on the view being visible.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, VecModel};

use super::track_change::apply_track_change;
use super::{NowPlayingState, UP_NEXT_N, current_track_id};
use crate::media::cover_thumbs::CoverThumbs;
use crate::player::state::QueueViewModel;
use crate::state::AppState;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::ui::queue_sheet::to_slint_queue_row;
use crate::{AppWindow, Nav, NowPlaying, QueueRow};

/// Subscribe to `sinks.queue`. While the view is closed the subscriber
/// only stashes the latest snapshot into `NowPlayingState::latest_qvm`;
/// while it's open it rebuilds the Up Next list — but only when the
/// visible id slice actually changed — and resets the
/// `NowPlaying.slide-phase` animation counter on actual current-track
/// changes.
pub(super) fn spawn_up_next_subscriber(
    ui: &AppWindow,
    state: &AppState,
    cover_thumbs: Arc<CoverThumbs>,
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

            // Skip the rebuild when nothing that renders the model is on
            // screen — neither the full-screen Now Playing view nor the
            // square miniplayer variant. Stash the snapshot so a later
            // open can rebuild from it.
            if !np_state.open.get() && !np_state.mini_visible.get() {
                *np_state.latest_qvm.borrow_mut() = Some(qvm);
                continue;
            }

            // View open: rebuild only when the visible slice actually
            // changed. A reorder/add *below* the Up Next window leaves the
            // rendered ids identical — diffing 20 freshly-built rows away
            // is pure waste. A current-track change always rebuilds (the
            // base index shifts) and additionally restarts the slide.
            let new_ids = upcoming_id_slice(&qvm);
            let current_id = current_track_id(&qvm);
            let track_changed = current_id != np_state.last_current_id.get();
            let ids_changed = *np_state.rendered_ids.borrow() != new_ids;
            // Snapshot the *old* visible ids before the rebuild overwrites
            // them — needed only on a track change, to look up the row
            // that fell off the bottom for the Backward outgoing overlay.
            let old_rendered_ids: Vec<i64> = if track_changed {
                np_state.rendered_ids.borrow().clone()
            } else {
                Vec::new()
            };
            if ids_changed || track_changed {
                let ids = rebuild_up_next(&ui, &cover_thumbs, &up_next_model, &qvm);
                *np_state.rendered_ids.borrow_mut() = ids;
            }
            if track_changed {
                let kind = classify_step(np_state.last_queue_index.get(), &qvm);
                // The track that fell off the bottom of the *old* visible
                // list — captured before the rebuild overwrote
                // `rendered_ids`. For a Backward step it becomes the
                // outgoing overlay row, but only if it actually fell off
                // (i.e. is no longer in the new visible slice — short
                // lists that grew by one don't drop anything).
                let dropped_tail_id = old_rendered_ids
                    .last()
                    .copied()
                    .filter(|id| !np_state.rendered_ids.borrow().contains(id));
                let outgoing_row =
                    outgoing_row(kind, &qvm, current_id, dropped_tail_id, &cover_thumbs);

                np_state.last_current_id.set(current_id);
                np_state.last_queue_index.set(qvm.queue_index);
                // Direct property writes — land in the same render as the
                // model swap above (one render follows both), so there's
                // no post-advance flash. Direction and outgoing-row are
                // set *before* the phase reset so the timer's first tick
                // already has the right offset and content.
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

/// Wire `Nav.now-playing-open-changed` (mirrored from the Slint side — see
/// `app-window.slint`). Updates the shared `open` flag and, on open, seeds
/// everything the two subscribers skipped while the view was closed.
pub(super) fn wire_now_playing_open(
    ui: &AppWindow,
    state: &AppState,
    cover_thumbs: Arc<CoverThumbs>,
    np_artwork: Arc<NowPlayingArtwork>,
    up_next_model: Rc<VecModel<QueueRow>>,
    np_state: Rc<NowPlayingState>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    ui.global::<Nav>().on_now_playing_open_changed(move |is_open| {
        np_state.open.set(is_open);
        if !is_open {
            // View closed — drop the decoded cover + blur buffers and hand
            // the pages back to the OS. The currently-displayed track's
            // buffers stay alive (the `Player` global still references the
            // `Image`s), so a same-track reopen still shows it without a
            // decode; only the LRU's other entries are freed. Off the UI
            // thread — `clear()` drops buffers and `trim()` walks arenas.
            let np_artwork = np_artwork.clone();
            state.runtime.spawn_blocking(move || {
                np_artwork.clear();
                crate::tasks::heap_trim::trim();
            });
            return;
        }
        let Some(ui) = weak.upgrade() else { return };

        // Up Next: rebuild from the latest snapshot stashed while closed.
        let latest_qvm = np_state.latest_qvm.borrow().clone();
        if let Some(qvm) = latest_qvm {
            let ids = rebuild_up_next(&ui, &cover_thumbs, &up_next_model, &qvm);
            *np_state.rendered_ids.borrow_mut() = ids;
            np_state.last_current_id.set(current_track_id(&qvm));
            // Sync the index too — re-opening with a track changed while
            // closed shouldn't replay a phantom animation, and the next
            // real change should compute direction against the current
            // index, not the one from before the view closed.
            np_state.last_queue_index.set(qvm.queue_index);
        }

        // Artwork + chips: seed for the current track, but only if it
        // differs from whatever is already written into the `Player`
        // global — a close/re-open with no track change in between needs
        // no decode. `animate = false`: the cover should already be there
        // when the view appears, not cross-fade in.
        let current_track = np_state.current_track.borrow().clone();
        let current_id = current_track.as_ref().map(|t| t.id);
        if current_id != np_state.applied_track_id.get() {
            let weak = weak.clone();
            let state = state.clone();
            let np_artwork = np_artwork.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                apply_track_change(&weak, &state, &np_artwork, &np_state, current_track, false)
                    .await;
            }));
            if let Err(e) = res {
                log::warn!("ui::now_playing open-seed task spawn_local: {e}");
            }
        }
    });
}

/// What kind of step the current-track change was. Drives both the
/// slide direction and which row (if any) gets rendered as the
/// transient "outgoing" overlay during the animation.
#[derive(Clone, Copy)]
enum SlideKind {
    /// Forward step (incl. wrap from last → first under repeat-all/one).
    /// The just-promoted track slides off the top with the list.
    Forward,
    /// Backward step (incl. wrap from first → last under repeat-all/one).
    /// The row that fell off the visible bottom slides off the bottom.
    Backward,
    /// Anything else (skip-to / direct play / queue rebuild). Animates
    /// forward by default; no outgoing overlay row.
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

/// Classify the transition from `old_idx` to `qvm.queue_index`,
/// recognising single-step forward / backward moves with wrap-around so
/// repeat-all and repeat-one navigation animates the right way.
fn classify_step(old_idx: i32, qvm: &QueueViewModel) -> SlideKind {
    let new_idx = qvm.queue_index;
    let len = i32::try_from(qvm.queue_tracks.len()).unwrap_or(i32::MAX);
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

/// Build the "outgoing" row for the slide animation, if any.
/// `Forward`: the just-promoted track (`current_id` — was at the top of
/// the old Up Next). `Backward`: `dropped_tail_id` — the track that
/// fell off the bottom of the old visible list (only set by the caller
/// when it actually fell off, never when the list merely grew).
/// `Other`: never.
fn outgoing_row(
    kind: SlideKind,
    qvm: &QueueViewModel,
    current_id: Option<i64>,
    dropped_tail_id: Option<i64>,
    cover_thumbs: &CoverThumbs,
) -> Option<QueueRow> {
    let id = match kind {
        SlideKind::Forward => current_id?,
        SlideKind::Backward => dropped_tail_id?,
        SlideKind::Other => return None,
    };
    let track = qvm.queue_tracks.iter().find(|t| t.id == id)?;
    let cover = cover_thumbs.get_or_load_opt(track.artwork_path.as_deref());
    Some(to_slint_queue_row(track.as_ref(), cover, false))
}

/// Play-order indices of the next `UP_NEXT_N` tracks after the current
/// one (current excluded), plus the play-order base index (`queue_index +
/// 1`, clamped; `queue_index` is -1 when nothing is playing → base 0).
///
/// With `RepeatMode::All` or `RepeatMode::One` the queue is a cycle: the
/// slice wraps past the end back to the start, stopping just before the
/// current track again. `RepeatMode::Off` uses a plain forward slice.
pub(super) fn upcoming_indices(qvm: &QueueViewModel) -> (Vec<usize>, usize) {
    let len = qvm.queue_tracks.len();
    let base = usize::try_from(qvm.queue_index + 1).unwrap_or(0);
    let wrap = qvm.repeat_mode.wraps() && qvm.queue_index >= 0;
    let indices: Vec<usize> = if wrap && len > 0 {
        (0..(len - 1).min(UP_NEXT_N))
            .map(|n| (base + n) % len)
            .collect()
    } else {
        (base..len.min(base + UP_NEXT_N)).collect()
    };
    (indices, base)
}

/// The track ids the Up Next list *would* render for `qvm` — the cheap
/// pre-check the subscriber uses to skip an unchanged rebuild.
fn upcoming_id_slice(qvm: &QueueViewModel) -> Vec<i64> {
    let (indices, _) = upcoming_indices(qvm);
    indices.iter().map(|&i| qvm.queue_tracks[i].id).collect()
}

/// Rebuild `up_next_model` with the next `UP_NEXT_N` tracks after the
/// current one and publish the play-order base index + queue length so row
/// clicks can `Queue.skip-to` (with a modulo for the wrapped repeat-all
/// case). Returns the rendered track-id slice for the caller's
/// `NowPlayingState::rendered_ids` shadow.
pub(super) fn rebuild_up_next(
    ui: &AppWindow,
    cover_thumbs: &CoverThumbs,
    up_next_model: &Rc<VecModel<QueueRow>>,
    qvm: &QueueViewModel,
) -> Vec<i64> {
    let (indices, base) = upcoming_indices(qvm);
    let mut ids = Vec::with_capacity(indices.len());
    let mut upcoming = Vec::with_capacity(indices.len());
    for &i in &indices {
        let t = qvm.queue_tracks[i].as_ref();
        ids.push(t.id);
        let cover_img = cover_thumbs.get_or_load_opt(t.artwork_path.as_deref());
        upcoming.push(to_slint_queue_row(t, cover_img, false));
    }

    crate::ui::model_diff::apply_rows_keyed(up_next_model, upcoming, |r| r.id);
    let np = ui.global::<NowPlaying>();
    np.set_base_index(i32::try_from(base).unwrap_or(i32::MAX));
    np.set_queue_length(i32::try_from(qvm.queue_tracks.len()).unwrap_or(i32::MAX));
    ids
}

