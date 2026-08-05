//! `MyLibrary.*` callbacks: the tab pick, the shared filter, the hero back button.
//!
//! Everything a *tab* does on entry and exit — release its cover tier, clear its models,
//! mark itself dirty, re-fetch — stays in that view's own lifecycle, driven by its
//! per-tab `SectionActiveGate`. What is left here is the page's own three handlers, none
//! of which needs a view handle, which is why this is wired straight after `wire_all`
//! rather than after the five `wire_*` calls.

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::my_library::{self as my_library_mod, MyLibraryTab};
use crate::{
    AlbumDetail, AppWindow, ArtistDetail, GenreDetail, MyLibrary, PlaylistDetail,
};

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
    // up filtered under an empty box. Dispatching the empty needle through the same
    // nine-way hand-off the box uses clears it *and* rebuilds that model from the cache
    // Rust already holds — synchronously, ahead of the section gate's re-fetch, so the
    // list is never blank and never stale.
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_tab_changed(move |tab| {
            if let Some(ui) = weak.upgrade() {
                let g = ui.global::<MyLibrary>();
                g.set_filter(SharedString::from(""));
                g.set_blur_search_tick(g.get_blur_search_tick() + 1);
                my_library_mod::filter::dispatch(&ui, "");
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

    // back: the band's back arrow, once it owns one. Routes to the mounted tab's own
    // `close-detail`, so every teardown that button already triggers — hero images,
    // cover tiers, `last_detail_ids`, the nav-history record — stays where it is.
    {
        let weak = weak.clone();
        g.on_back(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<MyLibrary>();
            match my_library_mod::tab_from_index(&g, g.get_tab_idx()) {
                MyLibraryTab::Songs => {}
                MyLibraryTab::Albums => ui.global::<AlbumDetail>().invoke_close_detail(),
                MyLibraryTab::Artists => ui.global::<ArtistDetail>().invoke_close_detail(),
                MyLibraryTab::Genres => ui.global::<GenreDetail>().invoke_close_detail(),
                MyLibraryTab::Playlists => {
                    ui.global::<PlaylistDetail>().invoke_close_detail();
                }
            }
        });
    }
}
