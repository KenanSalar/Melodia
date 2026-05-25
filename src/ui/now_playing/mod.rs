//! Full-screen Now Playing view wiring.
//!
//! Owns three pieces of Rust→Slint glue, all installed by [`install`] on
//! the Slint event-loop thread:
//!
//! 1. **Dual-slot blurred background + sharp cover.** A `sinks.view_model`
//!    subscriber detects track changes and — *only while the view is
//!    open* — decodes + blurs the new cover (off-thread, via
//!    [`crate::ui::now_playing_artwork`] — one decode yields both the
//!    blurred backdrop and the sharp foreground tile), writes it into the
//!    *inactive* of the two `Player.blur-img-{a,b}` slots, then flips
//!    `Player.blur-use-a`.
//!    Slint animates the two slots' opacity → a flash-free cross-fade.
//! 2. **Technical-metadata chips.** Same subscriber async-fetches the
//!    `TrackMeta` projection and writes a pre-formatted `TrackMetaRow`
//!    into `Player.track-meta`.
//! 3. **"Up Next" list.** A `sinks.queue` subscriber rebuilds the
//!    `NowPlaying.up-next-rows` `VecModel` (next N tracks in play order)
//!    and resets `NowPlaying.slide-phase` to 0 only when the current
//!    track actually changed.

mod metadata;
mod track_change;
mod up_next;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::entities::track::TrackSummary;
use crate::media::cover_thumbs::CoverThumbs;
use crate::player::state::{QueueViewModel, lock_state};
use crate::state::AppState;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::{AppWindow, Nav, NowPlaying, Player, QueueRow};

use track_change::spawn_track_change_subscriber;
use up_next::{rebuild_up_next, spawn_up_next_subscriber, wire_now_playing_open};

/// How many upcoming tracks to surface in the "Up Next" list. The list is
/// scrollable, so this is a soft cap — large enough to feel complete,
/// small enough that rebuilding it on every queue mutation stays cheap.
pub(crate) const UP_NEXT_N: usize = 20;

/// Shared UI-thread state coordinating the Now Playing view's two
/// subscribers and the `now-playing-open` callback. All three run on the
/// Slint event-loop thread, so `Rc<Cell/RefCell>` is enough — no atomics.
pub struct NowPlayingState {
    /// Mirrors `Nav.now-playing-open`. Both subscribers skip their work
    /// while this is false — nothing they produce is on screen.
    pub(super) open: Cell<bool>,
    /// Mirrors `MiniPlayer.active && MiniPlayer.square` — true while the
    /// square miniplayer variant is visible (the variant that renders the
    /// Up Next list). The up-next subscriber gates on `open || mini_visible`
    /// so the same model serves both surfaces without a parallel
    /// subscriber. Written from the `MiniPlayer.active-changed` callback
    /// in `crate::ui::mini_player`.
    pub(crate) mini_visible: Cell<bool>,
    /// Latest queue snapshot, kept whether or not the view is open, so
    /// opening the view can rebuild the Up Next list immediately.
    pub(super) latest_qvm: RefCell<Option<QueueViewModel>>,
    /// Track ids the Up Next model currently holds. A queue mutation that
    /// doesn't change this slice (e.g. a reorder *below* the Up Next
    /// window) skips the rebuild entirely.
    pub(super) rendered_ids: RefCell<Vec<i64>>,
    /// Current-track id at the last Up Next rebuild; a change drives the
    /// slide animation. `None` until the first populate.
    pub(super) last_current_id: Cell<Option<i64>>,
    /// Queue play-order index at the last Up Next rebuild. Compared
    /// against the new index to pick the slide direction: forward step
    /// (incl. wrap from last → first under repeat-all/one) slides up,
    /// backward step slides down. Seeded from the install snapshot so
    /// the first real track change picks the right direction.
    pub(super) last_queue_index: Cell<i32>,
    /// Latest current track from `sinks.view_model`, kept whether or not
    /// the view is open so opening it can seed the artwork + chips.
    pub(super) current_track: RefCell<Option<Arc<TrackSummary>>>,
    /// Track id whose artwork + metadata chips are currently written into
    /// the `Player` global. `None` until the first seed. The open callback
    /// compares it against `current_track` to skip a redundant re-seed
    /// when the track didn't change while the view was closed.
    pub(super) applied_track_id: Cell<Option<i64>>,
    /// Latest visible chip texts (in declared order) for the currently
    /// applied `track-meta`. Updated by the track-change subscriber after
    /// `set_track_meta`; re-chunked on every
    /// `Player.recompute-chip-rows(width)` fire so a window resize doesn't
    /// need to re-walk `TrackMetaRow`.
    pub(super) chip_texts: RefCell<Vec<SharedString>>,
    /// Last chip-area width reported by the view. Cached so the
    /// track-change subscriber can chunk against the current layout
    /// immediately, without waiting for the next Slint `changed` fire.
    pub(super) chip_last_width: Cell<f32>,
}

/// Install the Now Playing view's models + subscribers. Runs on the Slint
/// event-loop thread, between `AppWindow::new()` and `app.run()`.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    cover_thumbs: &Arc<CoverThumbs>,
    np_artwork: &Arc<NowPlayingArtwork>,
) -> Result<Rc<NowPlayingState>, slint::EventLoopError> {
    let up_next_model: Rc<VecModel<QueueRow>> = Rc::new(VecModel::default());
    ui.global::<NowPlaying>()
        .set_up_next_rows(ModelRc::from(up_next_model.clone()));

    // Snapshot the current state once. Subscribers' `watch::Receiver::changed()`
    // only fires on sends *after* subscribe, and the startup queue-restore
    // already broadcast — so without an explicit seed the view would be
    // empty until the next playback transition. Same rationale as
    // `queue_sheet::install`.
    let (current_track, qvm) = {
        let s = lock_state(&state.player_state);
        (s.to_view_model_light().current_track, s.to_queue_view_model())
    };
    let initial_track_id = current_track.as_ref().map(|t| t.id);

    // Shared state coordinating the `now-playing-open` callback and both
    // subscribers. Seeded `open` from the live Slint property (false at
    // startup), `last_current_id` from the snapshot so the first real
    // track change slides correctly, and `current_track` from the snapshot
    // so the first open can seed the artwork. `applied_track_id` starts
    // `None` — nothing is written into the `Player` global's artwork slots
    // yet, so the first open always seeds.
    let np_state = Rc::new(NowPlayingState {
        open: Cell::new(ui.global::<Nav>().get_now_playing_open()),
        mini_visible: Cell::new(false),
        latest_qvm: RefCell::new(None),
        rendered_ids: RefCell::new(Vec::new()),
        last_current_id: Cell::new(current_track_id(&qvm)),
        last_queue_index: Cell::new(qvm.queue_index),
        current_track: RefCell::new(current_track),
        applied_track_id: Cell::new(None),
        chip_texts: RefCell::new(Vec::new()),
        chip_last_width: Cell::new(0.0),
    });

    spawn_track_change_subscriber(
        ui,
        state,
        np_artwork.clone(),
        np_state.clone(),
        initial_track_id,
    )?;
    spawn_up_next_subscriber(
        ui,
        state,
        cover_thumbs.clone(),
        up_next_model.clone(),
        np_state.clone(),
    )?;
    wire_now_playing_open(
        ui,
        state,
        cover_thumbs.clone(),
        np_artwork.clone(),
        up_next_model.clone(),
        np_state.clone(),
    );

    // Chip-strip width sync. The view fires `recompute-chip-rows(width)` on
    // mount + every chip-area resize; we cache the width on
    // `chip_last_width` so the track-change subscriber can re-chunk against
    // the current layout without waiting for the next Slint `changed` fire.
    {
        let weak = ui.as_weak();
        let np = np_state.clone();
        ui.global::<Player>().on_recompute_chip_rows(move |width| {
            np.chip_last_width.set(width);
            let Some(ui) = weak.upgrade() else { return };
            let rows = metadata::chunk_chips_to_rows(&np.chip_texts.borrow(), width);
            ui.global::<Player>()
                .set_chip_rows(metadata::rows_to_model(rows));
        });
    }

    // Seed the Up Next list + the shared shadows synchronously (the
    // queue-restore broadcast already fired before the subscriber
    // subscribed), then hand the snapshot to `latest_qvm` so a later open
    // can rebuild from it. Same rationale as `queue_sheet::install`.
    let seeded_ids = rebuild_up_next(ui, cover_thumbs, &up_next_model, &qvm);
    *np_state.rendered_ids.borrow_mut() = seeded_ids;
    *np_state.latest_qvm.borrow_mut() = Some(qvm);

    // No artwork seed here: the blurred background + sharp cover + metadata
    // chips are decoded on demand by `wire_now_playing_open` the first time
    // the view is opened.
    Ok(np_state)
}

/// Write one dual-slot cross-fade pair — the blurred backdrop or the sharp
/// cover tile — into the `Player` global.
///
/// `animate = true` (a live track change while the view is open): write the
/// freshly-decoded image into the *inactive* slot, then flip `use_a`. The
/// previously-active slot keeps its image and stays painted for the whole
/// fade, so there's no empty frame mid-cross-fade.
///
/// `animate = false` (the seed-on-open path): write the *currently active*
/// slot in place — no flip, no opacity change — so the cover is simply
/// already there when the view appears.
///
/// `None` (no artwork, or a cover that failed to decode) clears `has_image`
/// instead: both slots fade to 0 and the accent-tinted gradient floor /
/// `music_note` placeholder shows through.
pub(crate) fn write_crossfade_slot(
    img: Option<Image>,
    animate: bool,
    use_a: bool,
    set_a: impl FnOnce(Image),
    set_b: impl FnOnce(Image),
    set_use_a: impl FnOnce(bool),
    set_has_image: impl FnOnce(bool),
) {
    match img {
        Some(img) => {
            match (animate, use_a) {
                // Live change: write the inactive slot, flip to it.
                (true, true) => {
                    set_b(img);
                    set_use_a(false);
                }
                (true, false) => {
                    set_a(img);
                    set_use_a(true);
                }
                // Seed: write the active slot in place, no flip.
                (false, true) => set_a(img),
                (false, false) => set_b(img),
            }
            set_has_image(true);
        }
        None => set_has_image(false),
    }
}

/// The id of the currently-playing track in a queue snapshot, if any.
pub(super) fn current_track_id(qvm: &QueueViewModel) -> Option<i64> {
    let idx = usize::try_from(qvm.queue_index).ok()?;
    qvm.queue_tracks.get(idx).map(|t| t.id)
}

#[cfg(test)]
#[path = "tests/now_playing_tests.rs"]
mod tests;
