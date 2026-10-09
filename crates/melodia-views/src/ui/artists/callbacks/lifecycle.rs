//! Artists section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch) and the `library_changed` subscriber
//! that keeps the grid + open detail fresh on watcher / scan events.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::detail_view::release_detail_hero_images;
use crate::ui::grid_prewarm;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::signal::on_signal;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::ui::track_detail::TrackDetail;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{
    AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, ArtistGridRow as UiArtistGridRow, Artists,
    CardSelection, TrackListRow as UiTrackListRow,
};

/// Wire the Artists section-lifecycle callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    artists_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Artists));
    // See the matching seed in `albums/lifecycle.rs`: a boot pre-fetch for a
    // section that isn't on screen can't publish the shared hero globals, so
    // its first enter has to re-fetch rather than take the cheap path.
    if !artists_ui.section_active() {
        artists_ui.mark_dirty();
    }
    wire_section_gate(ui, state, artists_ui);
    install_library_refresher(ui, state, artists_ui);
}

/// On leave: synchronously wipe the Slint `grid-rows` model + every
/// `ArtistDetail.*` property holding heavy refs (cover/blur Images,
/// tracks + albums + selected-ids `VecModel`s) on the UI thread so the
/// `SharedPixelBuffer` Arcs + `SharedString` allocations drop. Then
/// off-thread call `release_section_state` (Rust-side caches + grid
/// data + detail tracks + the shared grid tier down to proxies +
/// `malloc_trim`), which covers the detail's Albums strip too — one tier
/// serves both. On return: [`reenter`].
fn wire_section_gate(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    let au = artists_ui.clone();
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Artists>().on_section_active_changed(move |active| {
        au.set_section_active(active);
        if !active {
            // Land synchronously before the release task spawns — see
            // `ArtistsUi::data_dirty` for the race details.
            au.mark_dirty();
        }
        if !active && let Some(ui) = weak.upgrade() {
            let g = ui.global::<Artists>();
            // Rewound on the same tick as the model it numbers;
            // `Albums.total-count`'s declaration argues the sentinel.
            g.set_total_count(UNFETCHED_COUNT);
            clear_vec_model::<UiArtistGridRow>(&g.get_grid_rows(), "artists: clear grid");

            let d = ui.global::<ArtistDetail>();
            release_detail_hero_images(&ui, &d);
            clear_vec_model::<UiTrackListRow>(&d.get_tracks(), "artists: clear detail tracks");
            clear_vec_model::<UiAlbumRow>(&d.get_albums(), "artists: clear detail albums");
            clear_vec_model::<i32>(&d.get_selected_ids(), "artists: clear detail selection");
            d.set_selection_anchor(-1);
            // The card grid's set goes back with the models it describes;
            // `card-selection.slint` argues why it cannot outlive the leave.
            ui.global::<CardSelection>().invoke_clear();
        }
        if active {
            s.runtime.spawn(reenter(s.clone(), au.clone(), weak.clone()));
        } else {
            let au = au.clone();
            s.runtime.spawn_blocking(move || au.release_section_state());
        }
    });
}

/// The section came back on screen: a full `fetch_grid` if data was wiped,
/// else just prewarm (initial enter after boot's pre-fetch). The detail
/// re-fetch (if `ArtistDetail.artist-id >= 0`) runs after the grid fetch so
/// the user lands back where they were.
async fn reenter(s: AppState, au: Arc<ArtistsUi>, weak: slint::Weak<AppWindow>) {
    if !au.take_dirty() {
        grid_prewarm::prewarm_off_thread(&au, ArtistsUi::prewarm_visible_covers).await;
        return;
    }
    let open_id = au.detail_artist_id();
    if let Err(e) = artists_ui_mod::fetch_grid(&s, &au, weak.clone()).await {
        log::warn!("artists::section_enter fetch_grid: {}", describe(&e));
    }
    if open_id >= 0
        && let Err(e) = artists_ui_mod::open_artist(
            &s,
            &au,
            weak.clone(),
            open_id,
            melodia_ui::NavEnterFrom::Right,
        )
        .await
    {
        log::warn!("artists::section_enter open_artist({open_id}): {}", describe(&e));
        // Detail re-fetch failed (artist deleted while
        // hidden); drop back to the grid rather than
        // stranding the user on an empty detail page.
        // Mirrors the Albums slice's own lifecycle wiring.
        au.clear_detail();
        // Handed back warm, as a failed restore's grid is.
        grid_prewarm::prewarm_off_thread(&au, ArtistsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            let g = ui.global::<ArtistDetail>();
            g.set_artist_id(-1);
            release_detail_hero_images(&ui, &g);
        });
    }
}

/// Re-fetch the grid + refresh an open detail on `library_changed`.
/// Preserves sort + selection in the detail (uses `refresh_detail`, not
/// `open_artist`).
fn install_library_refresher(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    let s = state.clone();
    let au = artists_ui.clone();
    let installed =
        on_signal(&state.library_changed, ui.as_weak(), "artists-library-changed", move |ui| {
            // Skip the in-place refresh when the section is hidden, but
            // mark the cached data dirty so the next section-enter
            // re-fetches from scratch. Without the `mark_dirty`, a
            // `library_changed` arriving while the section was never
            // visited (e.g. the first scan after a fresh-DB launch) would
            // be lost. Mirrors the same gate in `albums/callbacks/lifecycle.rs`.
            if !au.section_active() {
                au.mark_dirty();
                return;
            }
            let open_id = au.detail_artist_id();
            {
                let s = s.clone();
                let au = au.clone();
                let weak = ui.as_weak();
                spawn_logged!(
                    s,
                    "artists::library_changed",
                    artists_ui_mod::fetch_grid(&s, &au, weak)
                );
            }
            if open_id >= 0 {
                let s = s.clone();
                let au = au.clone();
                let weak = ui.as_weak();
                spawn_logged!(
                    s,
                    "artists::library_changed_detail",
                    artists_ui_mod::refresh_detail(&s, &au, weak, open_id)
                );
            }
        });
    if let Err(e) = installed {
        log::warn!("Artists won't follow library changes: {}", describe(&e));
    }
}
