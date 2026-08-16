//! `MyLibrary.*` callbacks: the tab pick, the shared filter, the hero back button, and the
//! two teardowns the hero rides.
//!
//! Everything a *tab* does on entry and exit stays in that view's own lifecycle, driven by
//! its per-tab `SectionActiveGate`. What is left is the page's own handlers, none of which
//! needs a view handle — which is why this is wired straight after `wire_all`.
//!
//! **The hero is the page's, not a tab's**, and that is what the last two handlers are
//! for. A tab leave holds it, nothing on this page clearing a detail id on a tab switch,
//! so the banner is still what the band is collapsing out of and morphs back into on the
//! next pick. Handing it back splits between `hero-collapsed` (per id, once the morph has
//! finished) and `page-active-changed` (all of it, once the page is gone).

use std::cell::Cell;

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::release_hero_slots;
use crate::ui::my_library as my_library_mod;
use crate::{AlbumDetail, AppWindow, ArtistDetail, MyLibrary, PlaylistDetail};

/// Write the active tab to `views.json` on the blocking pool. The Slint property is
/// already correct by the time any caller gets here, so this is pure catch-up.
fn persist_tab(state: &AppState, tab: i32) {
    let s = state.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_my_library_tab(&s, tab) {
            log::warn!("my_library: set_my_library_tab({tab}): {e}");
        }
    });
}

/// Hand back what the band's hero was holding, once its collapse has finished.
///
/// **Deferred out of the four `close-detail` handlers.** Every hero fact is a ternary over
/// the detail id at the mount sheet, so tearing down when that id clears leaves the band
/// collapsing a placeholder glyph over a reset gradient for the whole morph.
///
/// **Which slots comes off the ids, because the band collapses for two reasons.** A close
/// cleared one and the banner behind it is nobody's; a tab switch out of a still-open
/// detail cleared nothing, and picking that tab again morphs the same banner straight back
/// open. All three globals are asked rather than the one that closed — the band can't say
/// which and doesn't need to. The colour set is [`release_page_hero`]'s for the same
/// reason, and the chip row asks the ids' question one layer down through
/// [`crate::ui::hero_chips::clear_if_stale`].
///
/// Safe unguarded: `LibraryTabBand` cancels its timer on a re-open, so this can only land
/// with no detail on screen.
fn release_collapsed_hero(ui: &AppWindow) {
    let album = ui.global::<AlbumDetail>();
    if album.get_album_id() < 0 {
        release_hero_slots!(album);
    }
    let artist = ui.global::<ArtistDetail>();
    if artist.get_artist_id() < 0 {
        release_hero_slots!(artist);
    }
    let playlist = ui.global::<PlaylistDetail>();
    if playlist.get_playlist_id() < 0 {
        release_hero_slots!(playlist);
    }
    crate::ui::hero_chips::clear_if_stale(ui);
}

/// Hand back every hero global the page owns, once the page itself is left.
///
/// **The one place the shared colour set is reset from My Library, and the only reach a
/// detail held on a tab you are not standing on has.** A tab leave keeps its hero and the
/// per-tab gates only fire for the *mounted* tab, so on a page leave the other four have
/// no edge left to deliver — hence a seam of the page's own, driven by the nav index;
/// `globals/my-library.slint` says why a sixth gate cannot deliver it either.
///
/// Unconditional, unlike [`release_collapsed_hero`]: past this point no id can bring a
/// banner back without a fetch that republishes all of it.
fn release_page_hero(ui: &AppWindow) {
    release_hero_slots!(ui.global::<AlbumDetail>());
    release_hero_slots!(ui.global::<ArtistDetail>());
    release_hero_slots!(ui.global::<PlaylistDetail>());
    crate::ui::hero_backdrop::reset(ui);
    crate::ui::hero_chips::clear(ui);
}

/// Wire the My Library page's own callbacks — reached through
/// [`super::install`]. Call once, after `wire_all`.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<MyLibrary>();
    let weak = ui.as_weak();

    // tab-changed: fires after the bar has moved `tab-idx` and the entering tab's gate has
    // re-fetched, so all the page owes is dropping the filter and remembering the pick.
    //
    // **Both sides, and the second is the entering tab's.** Clearing the band's box leaves
    // the property Rust filters by untouched, so the tab the pick lands on comes up
    // filtered under an empty box; `filter::clear_mounted` drops that needle through the
    // same nine-way hand-off.
    //
    // **A pick is also the one tab move that belongs in the back/forward history**, the
    // moves made on the user's behalf going through `persist-tab-idx` below.
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_tab_changed(move |tab| {
            if let Some(ui) = weak.upgrade() {
                let g = ui.global::<MyLibrary>();
                g.set_filter(SharedString::from(""));
                g.set_blur_search_tick(g.get_blur_search_tick() + 1);
                my_library_mod::filter::clear_mounted(&ui);
                crate::ui::nav_history::record_current(&s, &ui);
            }
            persist_tab(&s, tab);
        });
    }

    // persist-tab-idx: the same write without the filter clear, for the paths that
    // move the tab on the user's behalf — a cross-tab drill, a Mouse-4/5 walk.
    {
        let s = state.clone();
        g.on_persist_tab_idx(move |tab| persist_tab(&s, tab));
    }

    // filter-changed: one box, nine destinations. See `ui::my_library::filter`.
    {
        let weak = weak.clone();
        g.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            my_library_mod::filter::dispatch(&ui, text.as_str());
        });
    }

    // detail-scope-changed: the same nine-way hand-off read backwards, for the arrivals
    // that swap the surface under the box with nobody typing. `filter::sync_box` argues
    // why each of the three needs it.
    {
        let weak = weak.clone();
        g.on_detail_scope_changed(move || {
            let Some(ui) = weak.upgrade() else { return };
            my_library_mod::filter::sync_box(&ui);
        });
    }

    // back: the band's arrow, routed to the mounted tab's own `close-detail` so every
    // teardown that button already triggers stays where it is. Only the hero moved, to
    // `hero-collapsed` below; the dispatch is shared with `nav_history`'s Mouse-4 step.
    {
        let weak = weak.clone();
        g.on_back(move || {
            let Some(ui) = weak.upgrade() else { return };
            let tab = {
                let g = ui.global::<MyLibrary>();
                my_library_mod::tab_from_index(&g, g.get_tab_idx())
            };
            my_library_mod::close_open_detail(&ui, tab);
        });
    }

    // hero-collapsed: the band is done shrinking, so the banner it was painting is nobody's
    // — for whichever details actually closed. See `release_collapsed_hero`.
    {
        let weak = weak.clone();
        g.on_hero_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            release_collapsed_hero(&ui);
        });
    }

    // page-active-changed: the nav index left the page, which the five per-tab gates can't
    // report — on a page leave only the mounted tab's fires. The index is re-read rather
    // than trusted from the argument, which keeps the handler honest about the one thing
    // that must not fire a teardown: covering the band is not leaving it.
    //
    // **And the edge is latched, because the seam it rides fires on every nav change.** A
    // `changed` handler cannot ask which index it moved *from*, so an unlatched teardown
    // runs on Search → Browse too — and `seed_detail_from_settings` writes each detail's
    // cover and blur pair at boot precisely *because* the page is hidden. Seeded from the
    // handler's own question, sound because `install_views` hydrates the nav index first.
    {
        let weak = weak.clone();
        let was_up = Cell::new(my_library_mod::the_band_is_up(ui));
        g.on_page_active_changed(move |_active| {
            let Some(ui) = weak.upgrade() else { return };
            let up = my_library_mod::the_band_is_up(&ui);
            if was_up.replace(up) && !up {
                release_page_hero(&ui);
            }
        });
    }
}
