//! `MyLibrary.*` callbacks: the tab pick, the shared filter, the hero back button, and the
//! two teardowns the hero rides.
//!
//! Everything a *tab* does on entry and exit — release its cover tier, clear its models,
//! mark itself dirty, re-fetch — stays in that view's own lifecycle, driven by its
//! per-tab `SectionActiveGate`. What is left here is the page's own handlers, none of
//! which needs a view handle, which is why this is wired straight after `wire_all` rather
//! than after the five `wire_*` calls.
//!
//! **The hero is the page's, not a tab's**, and that is what the last two handlers are for.
//! A tab leave holds it — `release_shared_hero!` and `release_detail_hero_images!` gate on
//! `my_library::the_band_is_up`, and the chip row on its own owner — because nothing on this
//! page clears a detail id on a tab switch, so the banner is still what the band is
//! collapsing out of and still what it morphs back into on the next pick. Handing it back is
//! split between `hero-collapsed` (per id, once the morph has finished) and
//! `page-active-changed` (all of it, once the page is gone).

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
/// **Deferred out of the four `close-detail` handlers**, which is where it used to run.
/// Every hero fact — the cover, the blur pair, the title, the chips, and through
/// `HeroBackdrop` the tile fill and the text tiers — is a ternary over the detail id at
/// the mount sheet, so tearing down when that id clears left the band collapsing a
/// placeholder glyph over a reset gradient for the whole 400 ms instead of the banner it
/// was collapsing out of. The sheet holds the *arm* across the same window; this holds the
/// data behind it.
///
/// **Which slots that is comes off the ids, because the band collapses for two different
/// reasons.** A close cleared one and the banner behind it is nobody's; a tab switch out of
/// a still-open detail cleared nothing, and picking that tab again morphs the same banner
/// straight back open — ahead of the re-fetch the pick kicks. All three globals are asked
/// rather than the one that closed, because the band can't say which and doesn't need to:
/// a `-1` id whose slots are already `Image::default()` costs three writes that land on
/// what is there.
///
/// The colour set is *not* handed back here for the same reason, and its teardown is the
/// page's — [`release_page_hero`]. The chip row asks the same question the ids answer, one
/// layer down: [`crate::ui::hero_chips::clear_if_stale`] holds a row whose owner is still
/// open, so a tab switch that collapsed the band keeps counts that are still true and the
/// re-pick morphs back open with them already there.
///
/// Safe to run unguarded because `LibraryTabBand` cancels its timer on a re-open, so this
/// can only land with no detail on screen.
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
/// detail held on a tab you are not standing on has.** A tab leave keeps its hero — that is
/// what stops a collapse painting a fallback glyph and a re-entered tab morphing open onto
/// one — and the per-tab gates only fire for the *mounted* tab, so on a page leave the
/// other four have no edge left to deliver. Hence a seam of the page's own, driven by the
/// nav index rather than by a `SectionActiveGate`; `globals/my-library.slint` says why a
/// sixth gate cannot deliver that edge either.
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

/// Wire the My Library page's own callbacks. Call once, after `wire_all`.
pub fn wire_my_library(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<MyLibrary>();
    let weak = ui.as_weak();

    // tab-changed: fires after the bar has already moved `tab-idx`. The entering tab's
    // gate has fired too, so its own lifecycle is already re-fetching; all the page owes
    // is dropping the filter (a Songs needle carried into the Albums grid would silently
    // hide cards) and remembering the pick.
    //
    // **Both sides, and the second one is the entering tab's.** Clearing the band's box
    // leaves the view Rust filters by untouched, so the tab the pick lands on would come
    // up filtered under an empty box — `filter::clear_mounted` drops that needle through
    // the same nine-way hand-off the box uses. It clears *only* a tab that has one: the
    // entering surface's cache was wiped by its own section leave, so a dispatch into an
    // unfiltered tab rebuilds from nothing and hands the four grids the empty-state pair
    // their leave had deliberately withheld. That argument lives on `clear_mounted`.
    //
    // **A pick is also the one tab move that belongs in the back/forward history**, and
    // it is the only one that records: the moves made on the user's behalf — a cross-tab
    // drill, a Mouse-4/5 step — go through `persist-tab-idx` below, and a replay is
    // suppressed besides. `NavEntry.tab` has always existed to tell two tabs of this page
    // apart; without this call nothing ever pushed one, so Mouse-4 walked straight past
    // every grid the user reached by picking a tab.
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

    // detail-scope-changed: the same nine-way hand-off read backwards. A drill-in, a back
    // or a tab move that isn't a pick swaps the surface the box describes without anyone
    // typing, so the box takes that surface's own filter — see
    // `ui::my_library::filter::sync_box`, which argues why each of the three needs it.
    {
        let weak = weak.clone();
        g.on_detail_scope_changed(move || {
            let Some(ui) = weak.upgrade() else { return };
            my_library_mod::filter::sync_box(&ui);
        });
    }

    // back: the band's back arrow. Routes to the mounted tab's own `close-detail`, so
    // every teardown that button already triggers — the cover tiers, `last_detail_ids`,
    // the origin restore, the nav-history record — stays where it is. The one piece that
    // moved is the hero, to `hero-collapsed` below. The dispatch itself is shared with
    // `nav_history`'s Mouse-4 step out of a detail.
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

    // hero-collapsed: the band is done shrinking, so the banner it was painting is finally
    // nobody's — for whichever details actually closed. See `release_collapsed_hero`.
    {
        let weak = weak.clone();
        g.on_hero_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            release_collapsed_hero(&ui);
        });
    }

    // page-active-changed: the nav index left the page, which the five per-tab gates can't
    // report — on a page leave only the mounted tab's fires. See `release_page_hero`.
    //
    // The index is re-read rather than trusted from the argument. Cheap, and it keeps the
    // handler honest about the one thing that must not fire a teardown: Now Playing or the
    // miniplayer *covering* the band is not leaving it, and the same detail is still open
    // underneath when the cover lifts.
    //
    // **And the edge is latched, because the seam it rides fires on every nav change.** A
    // `changed` handler cannot ask which index it moved *from*, so an unlatched teardown
    // runs on Search → Browse too — and `seed_detail_from_settings` writes each detail's
    // cover and blur pair at boot precisely *because* the page is hidden, so that the first
    // visit paints instead of waiting on a re-fetch. One lateral nav would hand all of that
    // back and leave the band morphing open on `ArtworkImage`'s fallback glyph. Seeded from
    // the same question the handler asks, which is sound because `install_views` hydrates
    // the nav index before this runs.
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
