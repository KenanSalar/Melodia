//! The Radio page — three tabs over the radio-browser.info directory, the stations the user kept,
//! and the ones they played.
//!
//! **One section handle for all three**, and one `SectionActiveGate` to match. My Library's
//! per-tab gates are what its history left it — five tabs that used to be five sidebar sections,
//! each with a hook of its own — where here a tab flip has to stay inside the page: Browse holds a
//! directory answer bought with a network round trip, and a per-tab gate would hand it back every
//! time the user glanced at their favorites.
//!
//! **Station history is a tab rather than a sort of the kept list**, and the reason is which rows
//! it can reach: playing a station out of Browse keeps it without starring it, so a favorites-only
//! list is the one place such a row could never be found, re-starred or deleted from.
//!
//! **The section leave is narrower than every other grid page's**: it drops the logo tier and
//! keeps every list. See [`browse`], [`kept`] and [`covers`].
//!
//! It is also the *only* trigger for `tasks::radio_logo_cache`, which is the one piece of teardown
//! here that reaches past the UI. The browsed-logo store grows only while this page is open, so a
//! leave is exactly when it stops, and the artwork sweep's own trigger is a library scan — which a
//! user who browses radio and never touches their music folders may not run for weeks.

mod browse;
mod callbacks;
mod covers;
mod detail;
mod facets;
mod filter;
mod history;
mod identity;
mod kept;
mod logos;
mod rows;
mod suggest;
mod tabs;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::media::cover_thumbs::CoverThumbs;
use crate::state::AppState;
use crate::ui::artwork_cache::BlurSpec;
use crate::ui::detail_artwork::DetailArtwork;
use crate::ui::section_state::SectionState;
use crate::ui::view_ctx::ViewCtx;
use crate::{AppWindow, Nav, Radio, RadioFacetRow, RadioStationGridRow, RadioSuggestionRow};

use browse::BrowseState;
use logos::LogoMemo;
use tabs::section_is_up;

pub use callbacks::files::wire as wire_files;
pub use covers::tune_cache_for_display;
pub use detail::{StationRef, open_station_with, seed_detail_from_settings};
pub use history::install as install_history;
pub use identity::{StationTile, station_tile};
pub use rows::{display_tags, station_has_row};
pub use tabs::{RadioTab, seed_tab, tab_from_index};

/// This page's `Nav.selected-index`, and the single definition of it.
pub const NAV_RADIO: i32 = 10;

/// Map a persisted `views.json` nav index onto a live one when Radio is switched off.
///
/// A **sibling** of [`crate::ui::my_library::fold_retired_nav_index`] rather than an arm
/// inside it: that one answers "this index was retired", which is true of a file forever,
/// where this one answers "this index is unreachable in this install" and flips with a
/// setting. Left unfolded, a boot with the switch off selects a router branch that is
/// gated away and paints nothing.
pub fn fold_disabled_nav_index(idx: i32, radio_enabled: bool) -> i32 {
    if idx == NAV_RADIO && !radio_enabled {
        crate::ui::my_library::NAV_MY_LIBRARY
    } else {
        idx
    }
}

/// Everything switching Radio off has to undo, past the row and the router branch that
/// simply stop being mounted.
///
/// Stated here rather than in the settings callback so the page owns its own teardown,
/// and because three of the four are only findable from this side: a walk back onto a page
/// that no longer routes, a selected index the router no longer has a branch for, and a
/// tooltip left naming a row that no longer exists.
pub fn disable(ui: &AppWindow, state: &AppState) {
    if let Err(e) = crate::library::playback::player_stop_station(&state.playback_ctx()) {
        log::warn!("radio: stop station on disable: {e}");
    }
    state.nav_history.lock().forget_section(NAV_RADIO);

    // Through the callback rather than by clearing the flag, so the persisted id and the hero's
    // images go the one way they ever go. It is the whole page that is being taken away; leaving
    // a station named in `views.json` would reopen it the next time the switch went back on.
    let radio = ui.global::<Radio>();
    if radio.get_detail_open() {
        radio.invoke_close_detail();
    }
    // The pages waiting on the other two tabs never reached `detail-open`, so nothing above can
    // see them. Nothing is owed but forgetting them: the page they belong to is gone, and the
    // one name `views.json` carries is the mounted tab's, which the close above already cleared.
    if let Some(radio_ui) = state.ui_handles.radio.lock().clone() {
        state.runtime.spawn_blocking(move || {
            detail::forget_all_seats(&radio_ui);
            radio_ui.release_detail_artwork();
        });
    }

    let nav = ui.global::<Nav>();
    // Through the boot path's own fold, so the two can't disagree about where 10 goes.
    // Settings being the only way to the switch keeps this quiet today, but the placeholder
    // fall-through excludes 10 deliberately: anything landing there with the switch down
    // paints an empty panel, and `origin-nav-index` is one drill away from being able to.
    let selected = nav.get_selected_index();
    let landed = fold_disabled_nav_index(selected, false);
    if landed != selected {
        nav.set_selected_index(landed);
        nav.invoke_persist_selected_index(landed);
    }

    // `SidebarItem` publishes its identity into the rail's tooltip channel and clears it on
    // hover-exit, so a row unmounted under the pointer leaves the pill up with nothing left to
    // retract it. Same backstop `changed watched-mini-render` already owes the rail.
    if nav.get_sidebar_tip_idx() == NAV_RADIO {
        nav.set_sidebar_tip_idx(-1);
    }
}

/// Drop the cached facet lists, so the chips rebuild against a setting that just changed.
///
/// Nothing else re-asks inside one session: the section's prime skips a list already held, and the
/// picker skips the fetch for a chip whose list is still in hand. Parking `facet-shown` is what
/// takes away that second shortcut.
pub fn forget_facets(ui: &AppWindow, state: &AppState) {
    if let Some(radio_ui) = state.ui_handles.radio.lock().clone() {
        *radio_ui.facet_index.lock() = facets::FacetIndex::default();
        radio_ui.facet_list.lock().take();
    }
    ui.global::<Radio>().set_facet_shown(-1);
}

/// Wire every `Radio.*` callback and hand back the page's handle.
///
/// Returned for the cover retune alone: every wired closure clones its own strong `Arc`, so the
/// wiring is what keeps the handle alive.
pub fn install(cx: ViewCtx<'_>) -> Arc<RadioUi> {
    install_models(cx.app);
    let radio_ui =
        Arc::new(RadioUi::new(section_is_up(cx.app), crate::ui::detail_artwork::blur_spec(cx.app)));
    // **A boot landing on another section owes the flag its own leave never set.**
    // `SectionState` starts clean so the boot pre-fetch wins the first enter, but a station page
    // restored from `views.json` while Radio is off screen publishes into neither shared global —
    // and with nothing marked, the first enter would consume no flag and never republish, leaving
    // the band on a hero with no chips and no backdrop. The four detail sections seed it the same
    // way.
    if !radio_ui.section_active() {
        radio_ui.mark_dirty();
    }
    // A scheduled logo decode landing has nothing the card binding read to change, so the tier
    // signals and the generation is what re-runs it. Never off `0`: a batch landing after a
    // leave cleared the tier would otherwise read as warm.
    crate::ui::cover_generation::notify_on_decode(&radio_ui.covers, cx.app, covers::repaint);
    callbacks::wire(cx.app, cx.state, &radio_ui);
    radio_ui
}

/// Hand the global its empty `VecModel`s. Every later write finds them by downcasting back, so a
/// property left on the declared default is a silent no-op: an unbound array is a model of its own
/// kind, `write_grid`'s downcast misses, and the grid stays empty with one warning per attempt.
fn install_models(ui: &AppWindow) {
    let g = ui.global::<Radio>();

    // One per grid — the three tabs draw the same card but each keeps its own rows.
    let browsed: Rc<VecModel<RadioStationGridRow>> = Rc::new(VecModel::default());
    g.set_browse_rows(ModelRc::from(browsed));

    let favorites: Rc<VecModel<RadioStationGridRow>> = Rc::new(VecModel::default());
    g.set_favorites_rows(ModelRc::from(favorites));

    let recent: Rc<VecModel<RadioStationGridRow>> = Rc::new(VecModel::default());
    g.set_recent_rows(ModelRc::from(recent));

    let facets: Rc<VecModel<RadioFacetRow>> = Rc::new(VecModel::default());
    g.set_facet_options(ModelRc::from(facets));

    let suggestions: Rc<VecModel<RadioSuggestionRow>> = Rc::new(VecModel::default());
    g.set_suggestions(ModelRc::from(suggestions));
}

/// Rust-side state for the Radio page.
pub struct RadioUi {
    /// Whether the page is on screen. **Seeded at wire time rather than left to the gate**, which
    /// fires on transitions only and whose `ChangeTracker` baselines silently inside
    /// `AppWindow::new()` — a section seeded wrong has no edge left to correct it.
    ///
    /// The dirty flag rides with it, and describes the **hero and nothing else**. What a leave
    /// hands back here is the logo tier and, where a station page is open, the hero's decoded
    /// images and the colours it published into the two globals six heroes share — the three
    /// grids and the directory page survive, which is this page's whole departure from the
    /// tabbed-page contract. The enter re-warms the tier unconditionally and rebuilds the hero
    /// only when the flag says it was given up.
    section: SectionState,
    /// The directory page on screen, and the query it answers.
    browse: Mutex<BrowseState>,
    /// The directory uuids this install has starred. Refreshed on section enter and flipped
    /// optimistically by the toggle, which is what lets the star respond on the click's own frame.
    ///
    /// Derived from the same fetch that fills [`Self::kept`], since a starred station *is* a kept
    /// one — a query of its own would be a second answer to keep true.
    starred: Mutex<HashSet<String>>,
    /// The logo every kept station already has, keyed on directory uuid.
    ///
    /// **A row answers a question the memo cannot.** The memo is keyed on the `favicon_url` a
    /// browse asked about, so it only ever holds what *that URL* returned this session; a station
    /// whose favicon 404s and whose logo was found on its own site has an `artwork_path` and no
    /// memo entry, and a page carrying it drew a monogram beside the two local tabs painting the
    /// real thing. Filled from the same fetch as [`Self::starred`], for the same reason.
    known_logos: Mutex<HashMap<String, String>>,
    /// The station ids the logo repair has already asked about this session.
    ///
    /// `kept::refresh` runs on a section enter, on every star flip — Browse's included — and on
    /// every removal, and the repair walks every logo-less row each time. A station that failed is
    /// a stored backoff, so the repeat buys two queries and the same answer; this is that answer
    /// held where a click cannot spend a round trip on it.
    healed: Mutex<HashSet<i64>>,
    /// The Favorites tab: everything starred, plus every station typed in by hand.
    kept: Mutex<kept::KeptState>,
    /// The Recently Played tab: every station with a play behind it, starred or not.
    recent: Mutex<kept::KeptState>,
    /// What this session knows about station logos, keyed on the URL they came from.
    logos: LogoMemo,
    /// The open picker's list, whole. Kept beside the Slint model because the picker's needle
    /// narrows it and Slint cannot filter an array, so every keystroke rebuilds the model from
    /// here rather than re-asking the facade across the runtime.
    facet_list: Mutex<Option<Arc<[crate::entities::radio::Facet]>>>,
    /// All four directory lists at once, filled by `facets::prime` on the first section enter.
    ///
    /// The sibling above is whichever list the open picker is narrowing and answers a question
    /// about one chip; this answers a question about the *needle*, which is why it has to hold
    /// every list and to be readable without an `.await` — the scope suggestions are recomputed on
    /// each settled keystroke, on the UI thread.
    facet_index: Mutex<facets::FacetIndex>,
    /// A station page per tab, since one opens from all three and a tab move must not evict what
    /// another is holding. `detail.rs` owns the shape.
    detail: Mutex<detail::DetailState>,
    /// Held by the station-detail writer for the length of its `views.json` write.
    ///
    /// **That is the ordering** — two `spawn_blocking` tasks have none of their own, and with a
    /// name written on every tab move a bounce queues alternating values that can land reversed,
    /// naming the station the restored tab is *not* showing. `IndexPersist`'s shape minus the
    /// atomic, which is what the tab index beside it takes instead: the value a writer reloads
    /// under this is an `Option<i64>` and already lives beside the seats.
    persist_writer: Mutex<()>,
    /// The titles the station currently playing has announced. Not the detail's, and deliberately
    /// outside it: the ring fills whether or not anybody has the page open, which is the only way
    /// it can be there to read when they do.
    history: Mutex<history::StationHistory>,
    /// The grid tier the cards decode into. Released by the section leave.
    covers: Arc<CoverThumbs>,
    /// The hero's own tier, at detail size and with its blur half. Separate from [`Self::covers`]
    /// for `AlbumsUi`'s reason: one is a page of small cards, the other a single large tile.
    detail_artwork: Arc<DetailArtwork>,
}

impl RadioUi {
    fn new(section_active: bool, hero_blur: Option<BlurSpec>) -> Self {
        let section = SectionState::new();
        section.set_active(section_active);
        Self {
            section,
            browse: Mutex::new(BrowseState::default()),
            starred: Mutex::new(HashSet::new()),
            known_logos: Mutex::new(HashMap::new()),
            healed: Mutex::new(HashSet::new()),
            kept: Mutex::new(kept::KeptState::default()),
            recent: Mutex::new(kept::KeptState::default()),
            logos: LogoMemo::new(),
            facet_list: Mutex::new(None),
            facet_index: Mutex::new(facets::FacetIndex::default()),
            detail: Mutex::new(detail::DetailState::default()),
            persist_writer: Mutex::new(()),
            history: Mutex::new(history::StationHistory::default()),
            covers: Arc::new(covers::new_tier()),
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
        }
    }

    /// Mark the hero stale. Written synchronously on the UI thread at section leave, before the
    /// release task is spawned.
    fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Give the hero's decode tier back. The Slint image slots are the leave's own.
    fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
    }

    /// Whether the page is the section on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Let the logo repair ask about a station again.
    ///
    /// [`Self::healed`] is what stops a refresh re-asking about a station whose backoff already
    /// answered, and a website or logo URL the user has just typed is precisely the new evidence
    /// that claim was made without. Without this the field they filled in has no effect until the
    /// next launch.
    fn forget_heal(&self, id: i64) {
        self.healed.lock().remove(&id);
    }

    /// Flip a station's star in the shadow the grid is built from.
    ///
    /// The optimistic half of the toggle, and its revert: the write is a round trip through
    /// `SQLite` and the star has to answer on the click's own frame.
    fn set_local_favorite(&self, station_uuid: &str, favorite: bool) {
        let mut starred = self.starred.lock();
        if favorite {
            starred.insert(station_uuid.to_owned());
        } else {
            starred.remove(station_uuid);
        }
    }
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
