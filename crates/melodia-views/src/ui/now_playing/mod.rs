//! Full-screen Now Playing view wiring.
//!
//! Three pieces of Rust→Slint glue, all installed by [`install`] on the event-loop
//! thread:
//!
//! 1. **Dual-slot blurred background + sharp cover.** A `sinks.view_model` subscriber
//!    decodes and blurs the source's artwork off-thread through
//!    [`crate::ui::now_playing_artwork`] — *only while the view is open*, one decode yielding
//!    both — into the *inactive* `Player.blur-img-{a,b}` slot, then flips `Player.blur-use-a`.
//!    A station's logo runs the same path: the tier is keyed on a path and knows nothing about
//!    entity kinds.
//! 2. **Technical-metadata chips**, off the same subscriber's `TrackMeta` fetch.
//! 3. **"Up Next" list.** A `sinks.queue` subscriber rebuilds `NowPlaying.up-next-rows`
//!    and resets `slide-phase` only when the current track actually changed.

mod metadata;
mod source_change;
mod up_next;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::ui::chips;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track::TrackSummary;
use melodia_engine::player::engine::now_playing::SourceId;
use melodia_engine::player::engine::state::{PlayerViewModelLight, QueueViewModel, lock_state};
use melodia_ui::{AppWindow, Nav, NowPlaying, Player, QueueRow};

use async_compat::Compat;

pub(crate) use source_change::republish_for_palette;
use source_change::{apply_source_change, spawn_source_change_subscriber};
use up_next::{rebuild_up_next, spawn_up_next_subscriber, wire_now_playing_open};

/// Re-seed a `NowPlayingState`-shadowed surface — the Up Next list or the high-res cover
/// slot — from the stashed snapshot. Set once by [`install`], called by
/// [`crate::ui::shell::mini_player::install`] when the miniplayer becomes visible: the
/// subscribers stash while no surface renders the model, so without these kicks a
/// never-opened session followed by a direct shrink-to-mini shows empty or stale content.
type Seeder = Box<dyn Fn()>;

/// The list scrolls, so this is a soft cap — large enough to feel complete, small enough
/// that rebuilding it on every queue mutation stays cheap.
pub(crate) const UP_NEXT_N: usize = 20;

/// What the view's artwork, backdrop tiers and chips describe.
///
/// One type for both kinds of source, because every consumer in this module asks the same three
/// questions: has it moved, what is there to decode, and is there a `tracks` row behind it. A
/// station answers the last with `None` — chips come off an eight-column projection of a row it
/// does not have.
#[derive(Clone)]
pub(super) struct NowPlayingSource {
    pub(super) key: SourceKey,
    pub(super) artwork_path: Option<String>,
    pub(super) track: Option<Arc<TrackSummary>>,
}

/// The identity a repaint dedupes on.
///
/// **A station is its stream URL and nothing else.** Its announced title moves several times an
/// hour and changes none of what this module produces — the labels are Slint bindings on
/// `Player.vm` — so folding the title in would re-decode the same logo per song.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SourceKey {
    Track(i64),
    Station(String),
}

impl SourceKey {
    /// Whether `id` is the source `held` already names. Asked on every player emit, so it
    /// compares against the borrowed form rather than building a key to compare with: only a
    /// genuine move is worth a `String`.
    pub(super) fn describes(held: Option<&Self>, id: Option<SourceId<'_>>) -> bool {
        match (held, id) {
            (None, None) => true,
            (Some(Self::Track(held)), Some(SourceId::Track(id))) => *held == id,
            (Some(Self::Station(held)), Some(SourceId::Station(url))) => held == url,
            _ => false,
        }
    }
}

impl NowPlayingSource {
    /// Project a published view model, or `None` when nothing is on the deck.
    pub(super) fn from_vm(vm: &PlayerViewModelLight) -> Option<Self> {
        let source = vm.source()?;
        // The row comes off the same arm as the key rather than off `vm` beside it: `source` has
        // already settled which half wins, and reading `current_track` independently is what would
        // hand the chips a `tracks` row under a station.
        let (key, track) = match source.id {
            SourceId::Track(id) => (SourceKey::Track(id), vm.current_track.clone()),
            SourceId::Station(stream_url) => (SourceKey::Station(stream_url.to_owned()), None),
        };
        Some(Self {
            key,
            track,
            artwork_path: source.artwork_path.map(str::to_owned),
        })
    }
}

/// Shared UI-thread state coordinating the view's two subscribers and the
/// `now-playing-open` callback. All three run on the event-loop thread, so
/// `Rc<Cell/RefCell>` is enough.
pub struct NowPlayingState {
    /// Mirrors `Nav.now-playing-open`. Both subscribers skip their work while it is
    /// false — nothing they produce is on screen.
    pub(super) open: Cell<bool>,
    /// Mirrors `MiniPlayer.active`, true for either variant. Only the square one renders
    /// Up Next, but the broader flag keeps the gate simple and the wasted
    /// horizontal-variant rebuild is a handful of rows.
    pub(crate) mini_visible: Cell<bool>,
    /// Mirrors `MiniPlayer.square`, the variant with the large artwork tile. The
    /// source-change subscriber gates its high-res decode on
    /// `open || (mini_visible && mini_square)`, so the rectangle variant — served from the
    /// row tier — doesn't pay for a decode it can't display.
    pub(crate) mini_square: Cell<bool>,
    /// Latest queue snapshot, kept whether or not the view is open so opening it can
    /// rebuild Up Next immediately.
    pub(super) latest_qvm: RefCell<Option<QueueViewModel>>,
    /// Track ids the Up Next model currently holds. A queue mutation that doesn't
    /// change this slice — a reorder *below* the window — skips the rebuild.
    pub(super) rendered_ids: RefCell<Vec<i64>>,
    /// Current-track id at the last Up Next rebuild; a change drives the slide.
    pub(super) last_current_id: Cell<Option<i64>>,
    /// Queue play-order index at the last rebuild; a forward step (wrap included) slides
    /// up. Seeded from the install snapshot so the first real change picks right.
    pub(super) last_queue_index: Cell<i32>,
    /// Latest source on the deck, kept whether or not the view is open so opening it can
    /// seed the artwork and chips.
    pub(super) current_source: RefCell<Option<NowPlayingSource>>,
    /// The source whose artwork and chips are currently in the `Player` global. The open
    /// callback compares it against `current_source` to skip a redundant re-seed when
    /// nothing changed while the view was closed.
    pub(super) applied_source: RefCell<Option<SourceKey>>,
    /// Visible chip texts in declared order for the applied `track-meta`, re-chunked on
    /// every `Player.recompute-chip-rows(width)` so a resize needn't re-walk
    /// `TrackMetaRow`.
    pub(super) chip_texts: RefCell<Vec<SharedString>>,
    /// Last width the `MetaChipStrip` reported, so the source-change subscriber can chunk
    /// against the current layout without waiting for the next `changed` fire.
    pub(super) chip_last_width: Cell<f32>,
    /// Row lengths of the split last handed to `Player.chip-rows` — see
    /// [`crate::ui::chips::split_shape`]. Only the width channel consults it; the
    /// source-change subscriber writes unconditionally, its chips having moved by
    /// definition, and records the shape on its way past.
    pub(super) chip_last_shape: RefCell<Vec<usize>>,
    /// `None` only between `Rc::new(…)` and [`install`]'s post-init writes. Captures a
    /// `Weak<NowPlayingState>` to avoid the `Rc → closure → Rc` cycle.
    up_next_seeder: RefCell<Option<Seeder>>,
    /// The high-res cover, accent and chips, invoked by [`Self::kick_artwork`] when the
    /// square miniplayer becomes visible so the sharp tile replaces the row-tier fallback
    /// without waiting for the next source change.
    artwork_seeder: RefCell<Option<Seeder>>,
}

impl NowPlayingState {
    /// Rebuild the Up Next list from the stashed queue snapshot, so the square miniplayer
    /// doesn't render an empty list while the subscriber's snapshot is fresh. A no-op
    /// before [`install`] returns, and when nothing has been stashed.
    pub(crate) fn kick_up_next(&self) {
        if let Some(seeder) = self.up_next_seeder.borrow().as_ref() {
            seeder();
        }
    }

    /// Decode the current track's high-res cover, accent and chips into the `Player`
    /// global, on the rectangle→square transition and on an entry straight into square.
    /// A no-op before [`install`] returns, and when the current track is already applied.
    pub(crate) fn kick_artwork(&self) {
        if let Some(seeder) = self.artwork_seeder.borrow().as_ref() {
            seeder();
        }
    }
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
    ui.global::<NowPlaying>().set_up_next_rows(ModelRc::from(up_next_model.clone()));

    // Lazy row covers against the shared row tier, the `RowCovers` shape
    // `boot::ui_setup` wires for the track lists. `QueueRow` carries no decoded image, so
    // this is where an Up Next thumbnail comes from — only for the rows on screen.
    {
        let covers = cover_thumbs.clone();
        ui.global::<NowPlaying>().on_request_cover(move |path| {
            covers.get_or_load_opt(Some(path.as_str()).filter(|s| !s.is_empty()))
        });
    }

    // `watch::Receiver::changed()` only fires on sends *after* subscribe and the
    // startup queue-restore already broadcast, so without an explicit seed the view is
    // empty until the next playback transition. As in `queue_sheet::install`.
    let (current_source, qvm) = {
        let s = lock_state(&state.player_state);
        (NowPlayingSource::from_vm(&s.to_view_model_light()), s.to_queue_view_model())
    };
    let initial_key = current_source.as_ref().map(|s| s.key.clone());

    // `last_current_id` from the snapshot so the first real track change slides the right
    // way, `current_source` so the first open can seed the artwork. `applied_source`
    // starts `None`: the `Player` global's artwork slots are empty, so the first open
    // always seeds.
    let np_state = Rc::new(NowPlayingState {
        open: Cell::new(ui.global::<Nav>().get_now_playing_open()),
        mini_visible: Cell::new(false),
        mini_square: Cell::new(false),
        latest_qvm: RefCell::new(None),
        rendered_ids: RefCell::new(Vec::new()),
        last_current_id: Cell::new(current_track_id(&qvm)),
        last_queue_index: Cell::new(qvm.queue_index),
        current_source: RefCell::new(current_source),
        applied_source: RefCell::new(None),
        chip_texts: RefCell::new(Vec::new()),
        chip_last_width: Cell::new(0.0),
        chip_last_shape: RefCell::new(Vec::new()),
        up_next_seeder: RefCell::new(None),
        artwork_seeder: RefCell::new(None),
    });

    spawn_source_change_subscriber(ui, state, np_artwork.clone(), np_state.clone(), initial_key)?;
    spawn_up_next_subscriber(ui, state, up_next_model.clone(), np_state.clone())?;
    wire_now_playing_open(ui, state, np_artwork.clone(), up_next_model.clone(), np_state.clone());

    // Cached on `chip_last_width` so the source-change subscriber can re-chunk against the
    // current layout without waiting for the next `changed` fire.
    {
        let weak = ui.as_weak();
        let np = np_state.clone();
        ui.global::<Player>().on_recompute_chip_rows(move |width| {
            np.chip_last_width.set(width);
            let Some(ui) = weak.upgrade() else { return };
            let rows = chips::chunk_chips_to_rows(&np.chip_texts.borrow(), width, None);
            // The chips can't have moved — only `source_change` writes them — and
            // `set_chip_rows` is a model reset, fired here per pointer motion of a drag.
            let shape = chips::split_shape(&rows);
            if *np.chip_last_shape.borrow() == shape {
                return;
            }
            *np.chip_last_shape.borrow_mut() = shape;
            ui.global::<Player>().set_chip_rows(chips::rows_to_model(rows));
        });
    }

    // Synchronously, the queue-restore broadcast having fired before the subscriber
    // subscribed; the snapshot then goes to `latest_qvm` for a later open.
    let seeded_ids = rebuild_up_next(ui, &up_next_model, &qvm);
    *np_state.rendered_ids.borrow_mut() = seeded_ids;
    *np_state.latest_qvm.borrow_mut() = Some(qvm);

    // `Weak<NowPlayingState>` to avoid the `Rc → closure → Rc` cycle; everything else
    // is cheap to clone.
    {
        let weak_ui = ui.as_weak();
        let up_next_model = up_next_model.clone();
        let weak_np = Rc::downgrade(&np_state);
        *np_state.up_next_seeder.borrow_mut() = Some(Box::new(move || {
            let Some(ui) = weak_ui.upgrade() else { return };
            let Some(np_state) = weak_np.upgrade() else {
                return;
            };
            up_next::seed_from_stash(&ui, &up_next_model, &np_state);
        }));
    }

    // `wire_now_playing_open`'s seed-on-open path: dedup against `applied_source`, then
    // an off-thread decode and UI-thread write. `animate = false` — the cover should
    // already be there when the square miniplayer paints, not cross-fade in.
    {
        let weak_ui = ui.as_weak();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        let weak_np = Rc::downgrade(&np_state);
        *np_state.artwork_seeder.borrow_mut() = Some(Box::new(move || {
            let Some(np_state) = weak_np.upgrade() else {
                return;
            };
            let current_source = np_state.current_source.borrow().clone();
            let current_key = current_source.as_ref().map(|s| s.key.clone());
            if current_key == *np_state.applied_source.borrow() {
                return;
            }
            let weak_ui = weak_ui.clone();
            let state = state.clone();
            let np_artwork = np_artwork.clone();
            let res = slint::spawn_local(Compat::new(async move {
                apply_source_change(
                    &weak_ui,
                    &state,
                    &np_artwork,
                    &np_state,
                    current_source,
                    false,
                )
                .await;
            }));
            if let Err(e) = res {
                log::warn!("ui::now_playing artwork seeder task spawn_local: {e}");
            }
        }));
    }

    // No artwork seed here: backdrop, cover and chips are decoded on demand by
    // `wire_now_playing_open` on first open, or by `kick_artwork` when the square
    // miniplayer first becomes visible.
    Ok(np_state)
}

/// Write one dual-slot cross-fade pair — the blurred backdrop or the sharp cover tile —
/// into the `Player` global.
///
/// `animate = true` is a live track change: write into the *inactive* slot, then flip
/// `use_a`, the previously-active slot staying painted for the whole fade. `animate =
/// false` is the seed-on-open path: write the *active* slot in place, so the cover is
/// already there when the view appears. `None` — no artwork or a failed decode — clears
/// `has_image` instead, both slots fading to 0 over the gradient floor.
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
                (true, true) => {
                    set_b(img);
                    set_use_a(false);
                }
                (true, false) => {
                    set_a(img);
                    set_use_a(true);
                }
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
