//! `Artists.*` grid callbacks: lazy cover lookup, re-chunk on column
//! count, client-side filter / sort, and the open-artist drill-in.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::artists::{self as artists_ui_mod, ArtistsUi};
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::{next_sort, persist_view_sort, persisted_sort};
use crate::ui::track_list_view::view_id;
use crate::{AppWindow, ArtistDetail, Artists};

/// Wire the `Artists` grid callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    let artists = ui.global::<Artists>();
    let weak = ui.as_weak();

    // Seed the grid's sort pill from the persisted `view_sort["artists"]`.
    if let Some((field, dir)) = persisted_sort(state, view_id::ARTISTS) {
        artists.set_sort_field(SharedString::from(field.as_str()));
        artists.set_sort_dir(SharedString::from(dir));
    }

    // request-cover: lazy per-card cover lookup. Backed by the Artists
    // grid-tier `CoverThumbs` LRU.
    {
        let au = artists_ui.clone();
        // `generation` is read for its effect on the binding, never its value.
        artists.on_request_cover(move |path, _generation| au.grid_cover(path.as_str()));
        crate::ui::cover_generation::notify_on_decode(&artists_ui.grid_thumbs(), ui, |app| {
            let artists = app.global::<Artists>();
            artists.set_covers_generation(artists.get_covers_generation().wrapping_add(1));
        });
    }

    {
        let au = artists_ui.clone();
        let weak = weak.clone();
        artists.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            artists_ui_mod::rebuild_grid(&ui, &au);
        });
    }

    {
        let au = artists_ui.clone();
        let weak = weak.clone();
        artists.on_apply_filter(move |_text| {
            let Some(ui) = weak.upgrade() else { return };
            artists_ui_mod::rebuild_grid(&ui, &au);
        });
    }

    {
        let s = state.clone();
        let au = artists_ui.clone();
        let weak = weak.clone();
        artists.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Artists>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            artists_ui_mod::rebuild_grid(&ui, &au);
            persist_view_sort(&s, view_id::ARTISTS, new_field, new_dir);
        });
    }

    // open-artist: a card click. Fetches the detail header + albums +
    // tracks and flips `ArtistDetail.artist-id >= 0`. Also stamps the
    // Artists entry in `settings.last_detail_ids`.
    {
        let s = state.clone();
        let au = artists_ui.clone();
        let weak = weak.clone();
        artists.on_open_artist(move |artist_id| {
            let id = i64::from(artist_id);

            // Same-tab open: defensively zero any stale cross-section origin —
            // see `albums::grid`'s copy for the path it guards against.
            if let Some(ui) = weak.upgrade() {
                ui.global::<ArtistDetail>().set_origin_nav_index(-1);
            }

            let s_fetch = s.clone();
            let au_fetch = au.clone();
            let weak_fetch = weak.clone();
            spawn_logged!(
                s_fetch,
                "artists::open_artist",
                artists_ui_mod::open_artist(
                    &s_fetch,
                    &au_fetch,
                    weak_fetch,
                    id,
                    crate::NavEnterFrom::Right
                )
            );

            let au_release = au.clone();
            s.runtime.spawn_blocking(move || au_release.release_grid_covers());

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::ARTIST_DETAIL,
                    Some(id),
                ) {
                    log::warn!("artists::open_artist persist: {e}");
                }
            });
        });
    }
}
