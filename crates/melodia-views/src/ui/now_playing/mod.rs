//! The Now Playing view and the state the miniplayer shares with it: the cover and its blurred
//! backdrop, decoded off-thread only while a surface draws them, the metadata chips, Up Next and
//! the lyrics panel. A station's logo takes the cover's path, the tier knowing nothing of kinds.

mod lyrics;
mod metadata;
mod source_change;
mod up_next;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Image, ModelRc, TimerMode, VecModel};

use crate::ui::now_playing_artwork::NowPlayingArtwork;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::describe;
use melodia_engine::player::engine::now_playing::SourceId;
use melodia_engine::player::engine::state::{PlayerViewModelLight, QueueViewModel, lock_state};
use melodia_ui::{AppWindow, MiniLayout, MiniPlayer, Nav, NowPlaying, Player, QueueRow, Theme};

use async_compat::Compat;

pub(crate) use source_change::republish_for_palette;
use source_change::{apply_source_change, spawn_source_change_subscriber};
use up_next::{spawn_seed, spawn_up_next_subscriber, wire_now_playing_open, wire_reorder};

/// Re-fills a surface `NowPlayingState` shadows, or hands the cover back. Set once [`install`] has
/// built the state each reads: the subscribers stash while nothing renders, so a session shrinking
/// straight to the miniplayer would otherwise show empty or stale content.
type Hook = Box<dyn Fn()>;

/// The list scrolls, so this is a soft cap — large enough to feel complete, small enough
/// that rebuilding it on every queue mutation stays cheap.
pub(crate) const UP_NEXT_N: usize = 20;

/// What the view's artwork, backdrop tiers and chips describe, for either kind of source. A
/// station has no `tracks` row, which the chips come off.
#[derive(Clone)]
pub(super) struct NowPlayingSource {
    pub(super) key: SourceKey,
    pub(super) artwork_path: Option<String>,
    pub(super) track: Option<Arc<TrackSummary>>,
}

/// The identity a repaint dedupes on. **A station is its stream URL alone**: its announced title
/// moves several times an hour and changes nothing produced here, so folding it in would re-decode
/// the logo per song.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SourceKey {
    Track(i64),
    Station(String),
}

impl SourceKey {
    /// Asked on every player emit, so it compares against the borrowed form: only a genuine move is
    /// worth a `String`.
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
    pub(super) fn from_vm(vm: &PlayerViewModelLight) -> Option<Self> {
        let source = vm.source()?;
        // Off the key's own arm: `source` has settled which half wins, and `current_track` read
        // beside it would hand the chips a `tracks` row under a station.
        let (key, track) = match source.id {
            SourceId::Track(id) => (SourceKey::Track(id), vm.current_track.clone()),
            SourceId::Station(stream_url) => (SourceKey::Station(stream_url.to_owned()), None),
        };
        Some(Self { key, track, artwork_path: source.artwork_path.map(str::to_owned) })
    }
}

/// The UI-thread state the view's subscribers, its open callback and the miniplayer share.
pub struct NowPlayingState {
    /// Mirrors `Nav.now-playing-open`; the subscribers skip their work while it is false.
    pub(super) open: Cell<bool>,
    /// Mirrors `MiniPlayer.active`, every layout: only the column renders Up Next, but the rebuild
    /// wasted behind the other two is a handful of rows.
    pub(crate) mini_visible: Cell<bool>,
    pub(crate) mini_layout: Cell<MiniLayout>,
    /// Mirrors `MiniPlayer.backdrop-shown`. The backdrop solves off the large artwork's decode, so
    /// even the strip needs that decode while this is on.
    pub(crate) mini_backdrop: Cell<bool>,
    /// Kept while the view is closed, so opening it rebuilds Up Next at once.
    pub(super) latest_qvm: RefCell<Option<QueueViewModel>>,
    /// A mutation leaving this slice alone, a reorder below the window, skips the rebuild.
    pub(super) rendered_ids: RefCell<Vec<i64>>,
    /// The newest Up Next fetch, so one overtaken by a later publish or seed lands nothing.
    pub(super) up_next_ticket: Cell<u64>,
    /// A change drives the slide.
    pub(super) last_current_id: Cell<Option<i64>>,
    /// A forward step, wrap included, slides up.
    pub(super) last_queue_index: Cell<i32>,
    /// Kept while the view is closed, so opening it can seed the artwork and chips.
    pub(super) current_source: RefCell<Option<NowPlayingSource>>,
    /// The source in the `Player` global, so an open with nothing changed skips the re-seed.
    pub(super) applied_source: RefCell<Option<SourceKey>>,
    up_next_seeder: RefCell<Option<Hook>>,
    artwork_seeder: RefCell<Option<Hook>>,
    artwork_releaser: RefCell<Option<Hook>>,
    /// Defers the release past the fades still reading the slots it clears. One timer rather than
    /// one per release, so a later release restarts the wait instead of racing it.
    slot_release: slint::Timer,
    /// Here rather than beside the `if`-mounted panel, which it outlives, and beside the three
    /// edges that refill it.
    pub(super) lyrics: Rc<lyrics::LyricsUi>,
}

impl NowPlayingState {
    /// Rebuilds Up Next from the stashed snapshot, for a miniplayer column that has just shown.
    pub(crate) fn kick_up_next(&self) {
        if let Some(seeder) = self.up_next_seeder.borrow().as_ref() {
            seeder();
        }
    }

    /// Decodes the source's cover and chips for a miniplayer surface that has just started drawing
    /// them, unless they are already applied.
    pub(crate) fn kick_artwork(&self) {
        if let Some(seeder) = self.artwork_seeder.borrow().as_ref() {
            seeder();
        }
    }

    /// Hands back the cover and blur once no surface draws them: the cross-fade slots not on show
    /// and the decode cache, after the fades still reading them are over. The shown slots stay, so
    /// a same-source reopen needs no decode.
    pub(crate) fn release_artwork(&self) {
        if let Some(releaser) = self.artwork_releaser.borrow().as_ref() {
            releaser();
        }
    }

    /// Asks the lyrics sheet to catch up with the current track; `reseed` dedupes on its claim.
    pub(crate) fn kick_lyrics(&self) {
        self.lyrics.kick();
    }

    pub(crate) fn renders_artwork(&self) -> bool {
        self.surfaces().renders_artwork()
    }

    pub(crate) fn renders_panel(&self) -> bool {
        self.surfaces().renders_panel()
    }

    fn surfaces(&self) -> Surfaces {
        Surfaces {
            open: self.open.get(),
            mini_visible: self.mini_visible.get(),
            mini_layout: self.mini_layout.get(),
            mini_backdrop: self.mini_backdrop.get(),
        }
    }
}

/// The mirrors of what is on screen, read together by the two render gates.
#[derive(Debug, Clone, Copy)]
struct Surfaces {
    open: bool,
    mini_visible: bool,
    mini_layout: MiniLayout,
    mini_backdrop: bool,
}

impl Surfaces {
    /// Whether anything draws the cover, the chips and the solved colour tiers: the backdrop counts,
    /// its colours coming off the same decode.
    fn renders_artwork(self) -> bool {
        let large_artwork = self.mini_layout != MiniLayout::Strip;
        self.open || (self.mini_visible && (large_artwork || self.mini_backdrop))
    }

    /// Whether anything mounts Up Next and the lyrics sheet, **deliberately narrower** than
    /// [`Self::renders_artwork`]: only the column has a slot for either, and a sheet resolved for
    /// the card or the strip would spend a request per track on nothing anyone can see.
    fn renders_panel(self) -> bool {
        self.open || (self.mini_visible && self.mini_layout == MiniLayout::Column)
    }
}

/// Drops the [`NowPlayingArtwork`] LRU and trims the heap, off the UI thread. After
/// [`clear_unshown_slots`], so the trim sees what both let go of.
fn release_artwork_off_thread(state: &AppState, np_artwork: &Arc<NowPlayingArtwork>) {
    let np_artwork = Arc::clone(np_artwork);
    state.runtime.spawn_blocking(move || {
        np_artwork.clear();
        melodia_platform::services::platform::allocator::trim();
    });
}

/// Hands back the lyrics sheet once no panel is left, and prunes the store for the radio logo
/// cache's reason: a mounted panel is its only writer, so the last one going is when it stops
/// growing.
pub(crate) fn release_lyrics(ui: &AppWindow, state: &AppState, np_state: &NowPlayingState) {
    lyrics::release(ui, &np_state.lyrics);
    if melodia_app::library::lyrics::is_enabled(state) {
        melodia_app::tasks::lyrics_cache::spawn(
            &melodia_app::tasks::TaskSpawner::from_state(state),
            state,
        );
    }
}

/// Runs between `AppWindow::new()` and `app.show()`.
pub fn install(
    ui: &AppWindow,
    state: &AppState,
    cover_thumbs: &Arc<CoverThumbs>,
    np_artwork: &Arc<NowPlayingArtwork>,
) -> Result<Rc<NowPlayingState>, slint::EventLoopError> {
    let up_next_model: Rc<VecModel<QueueRow>> = Rc::new(VecModel::default());
    ui.global::<NowPlaying>().set_up_next_rows(ModelRc::from(up_next_model.clone()));
    wire_request_cover(ui, cover_thumbs);

    // Seeded from the state: the startup queue restore broadcast before anything subscribed.
    let (current_source, qvm) = {
        let s = lock_state(&state.player_state);
        (NowPlayingSource::from_vm(&s.to_view_model_light()), s.to_queue_view_model())
    };
    let initial_key = current_source.as_ref().map(|s| s.key.clone());
    let np_state = Rc::new(NowPlayingState::new(ui, state, current_source, &qvm));

    spawn_source_change_subscriber(ui, state, np_artwork.clone(), np_state.clone(), initial_key)?;
    spawn_up_next_subscriber(ui, state, up_next_model.clone(), np_state.clone())?;
    wire_now_playing_open(ui, state, np_artwork.clone(), up_next_model.clone(), np_state.clone());
    wire_reorder(ui, state, up_next_model.clone(), np_state.clone());

    // Kept for a later open too; the rows land once the loop runs.
    *np_state.latest_qvm.borrow_mut() = Some(qvm);
    spawn_seed(ui.as_weak(), state.clone(), up_next_model.clone(), np_state.clone());

    // Each hook holds a `Weak<NowPlayingState>`, against an `Rc` cycle through its own field.
    install_up_next_seeder(ui, state, &up_next_model, &np_state);
    install_artwork_seeder(ui, state, np_artwork, &np_state);
    install_artwork_releaser(ui, state, np_artwork, &np_state);

    lyrics::wire_reseed(ui, state, &np_state);
    lyrics::wire_menu(ui, state, &np_state);

    // No artwork seed: it decodes on the first open, or on `kick_artwork`.
    Ok(np_state)
}

impl NowPlayingState {
    /// `applied_source` starts `None`, the global's slots being empty, so the first open seeds.
    fn new(
        ui: &AppWindow,
        state: &AppState,
        current_source: Option<NowPlayingSource>,
        qvm: &QueueViewModel,
    ) -> Self {
        Self {
            open: Cell::new(ui.global::<Nav>().get_now_playing_open()),
            mini_visible: Cell::new(false),
            mini_layout: Cell::new(MiniLayout::Strip),
            // Off the global `hydrate_ui_from_settings` already seeded. The switch writes the two
            // mirrors beside it on entry.
            mini_backdrop: Cell::new(ui.global::<MiniPlayer>().get_backdrop_shown()),
            latest_qvm: RefCell::new(None),
            rendered_ids: RefCell::new(Vec::new()),
            up_next_ticket: Cell::new(0),
            last_current_id: Cell::new(current_track_id(qvm)),
            last_queue_index: Cell::new(qvm.queue_index),
            current_source: RefCell::new(current_source),
            applied_source: RefCell::new(None),
            up_next_seeder: RefCell::new(None),
            artwork_seeder: RefCell::new(None),
            artwork_releaser: RefCell::new(None),
            slot_release: slint::Timer::default(),
            lyrics: lyrics::install(ui, state),
        }
    }
}

/// `QueueRow` carries no image, so an Up Next thumbnail comes from the row tier here, for the rows
/// on screen only.
fn wire_request_cover(ui: &AppWindow, cover_thumbs: &Arc<CoverThumbs>) {
    let covers = cover_thumbs.clone();
    ui.global::<NowPlaying>().on_request_cover(move |path| {
        covers.get_or_load_opt(Some(path.as_str()).filter(|s| !s.is_empty()))
    });
}

fn install_up_next_seeder(
    ui: &AppWindow,
    state: &AppState,
    up_next_model: &Rc<VecModel<QueueRow>>,
    np_state: &Rc<NowPlayingState>,
) {
    let weak_ui = ui.as_weak();
    let state = state.clone();
    let up_next_model = up_next_model.clone();
    let weak_np = Rc::downgrade(np_state);
    *np_state.up_next_seeder.borrow_mut() = Some(Box::new(move || {
        let Some(np_state) = weak_np.upgrade() else {
            return;
        };
        spawn_seed(weak_ui.clone(), state.clone(), up_next_model.clone(), np_state);
    }));
}

/// Not animated: the cover should already be there when the miniplayer paints.
fn install_artwork_seeder(
    ui: &AppWindow,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &Rc<NowPlayingState>,
) {
    let weak_ui = ui.as_weak();
    let state = state.clone();
    let np_artwork = np_artwork.clone();
    let weak_np = Rc::downgrade(np_state);
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
            apply_source_change(&weak_ui, &state, &np_artwork, &np_state, current_source, false)
                .await;
        }));
        if let Err(e) = res {
            log::warn!("ui::now_playing artwork seeder task spawn_local: {}", describe(&e));
        }
    }));
}

/// Waits out `dur-med`, which the cover's, the blur's and the miniplayer backdrop's fades run on,
/// and hands nothing back if a surface draws again by then.
fn install_artwork_releaser(
    ui: &AppWindow,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &Rc<NowPlayingState>,
) {
    let weak_ui = ui.as_weak();
    let state = state.clone();
    let np_artwork = np_artwork.clone();
    let weak_np = Rc::downgrade(np_state);
    *np_state.artwork_releaser.borrow_mut() = Some(Box::new(move || {
        let (Some(ui), Some(np_state)) = (weak_ui.upgrade(), weak_np.upgrade()) else {
            return;
        };
        let fade_ms = u64::try_from(ui.global::<Theme>().get_dur_med()).unwrap_or(0);
        let weak_ui = weak_ui.clone();
        let weak_np = weak_np.clone();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        np_state.slot_release.start(
            TimerMode::SingleShot,
            Duration::from_millis(fade_ms),
            move || {
                let (Some(ui), Some(np_state)) = (weak_ui.upgrade(), weak_np.upgrade()) else {
                    return;
                };
                if np_state.renders_artwork() {
                    return;
                }
                clear_unshown_slots(&ui.global::<Player>());
                release_artwork_off_thread(&state, &np_artwork);
            },
        );
    }));
}

/// Writes one cross-fade pair, the blurred backdrop or the sharp cover.
///
/// Animated, the image goes into the inactive slot and `use_a` flips, the outgoing slot staying
/// painted for the fade. Not animated, the active slot is written in place, so the cover is there
/// when the view appears. `None` clears `has_image`, both slots fading over the gradient floor.
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

/// Empties every cross-fade slot nothing draws: the one `use_a` doesn't name, or both where there
/// is no image. Only once no fade is reading them.
fn clear_unshown_slots(player: &Player<'_>) {
    clear_unshown_slot(
        player.get_np_cover_has_image(),
        player.get_np_cover_use_a(),
        |img| player.set_np_cover_a(img),
        |img| player.set_np_cover_b(img),
    );
    clear_unshown_slot(
        player.get_blur_has_image(),
        player.get_blur_use_a(),
        |img| player.set_blur_img_a(img),
        |img| player.set_blur_img_b(img),
    );
}

fn clear_unshown_slot(
    has_image: bool,
    use_a: bool,
    set_a: impl FnOnce(Image),
    set_b: impl FnOnce(Image),
) {
    match (has_image, use_a) {
        (true, true) => set_b(Image::default()),
        (true, false) => set_a(Image::default()),
        (false, _) => {
            set_a(Image::default());
            set_b(Image::default());
        }
    }
}

pub(super) fn current_track_id(qvm: &QueueViewModel) -> Option<i64> {
    let idx = usize::try_from(qvm.queue_index).ok()?;
    qvm.ids.get(idx).copied()
}

#[cfg(test)]
#[path = "tests/now_playing_tests.rs"]
mod tests;
