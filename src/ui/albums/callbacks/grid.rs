//! `Albums.*` grid callbacks: lazy cover lookup, re-chunk on column count,
//! client-side filter / sort, and the open-album drill-in.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::state::AppState;
use crate::ui::albums::{self as albums_ui_mod, AlbumsUi};
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::callbacks::{next_sort, persist_view_sort, persisted_sort};
use crate::ui::track_list_view::view_id;
use crate::{AlbumDetail, Albums, AppWindow};

/// Wire the `Albums` grid callbacks. See [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let albums = ui.global::<Albums>();
    let weak = ui.as_weak();

    // Seed the grid's sort pill from the persisted `view_sort["albums"]`
    // so the initial grid build (and the pill) use the remembered order.
    if let Some((field, dir)) = persisted_sort(state, view_id::ALBUMS) {
        albums.set_sort_field(SharedString::from(field.as_str()));
        albums.set_sort_dir(SharedString::from(dir));
    }

    // request-cover: lazy per-card cover lookup. Only on-screen
    // `EntityCard`s invoke this, so off-screen albums never decode or
    // lock a cover. Backed by the shared `CoverThumbs` LRU. The Album
    // Detail **header** no longer uses a lazy callback — its cover is
    // decoded paired with the hero blur and pushed into
    // `AlbumDetail.cover` at `open_album` time (see `albums.rs`).
    {
        let au = albums_ui.clone();
        albums.on_request_cover(move |path| au.grid_cover(path.as_str()));
    }

    // columns-changed: the view recomputed its integer column count and
    // already wrote `Albums.columns`. Re-chunk the cached list — no DB hit.
    {
        let au = albums_ui.clone();
        let weak = weak.clone();
        albums.on_columns_changed(move |_cols| {
            let Some(ui) = weak.upgrade() else { return };
            albums_ui_mod::rebuild_grid(&ui, &au);
        });
    }

    // apply-filter: client-side; `Albums.filter` is already updated via the
    // SearchBar's two-way binding, so just rebuild.
    {
        let au = albums_ui.clone();
        let weak = weak.clone();
        albums.on_apply_filter(move |_text| {
            let Some(ui) = weak.upgrade() else { return };
            albums_ui_mod::rebuild_grid(&ui, &au);
        });
    }

    // request-sort: clicking a sort pill. Same field flips dir; a new field
    // resets to ascending. Albums sort in-memory (the DB query is fixed
    // name-ASC) — no DB round-trip.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        albums.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Albums>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            albums_ui_mod::rebuild_grid(&ui, &au);
            persist_view_sort(&s, view_id::ALBUMS, new_field, new_dir);
        });
    }

    // open-album: a card click. Fetches the detail header + track list and
    // flips `AlbumDetail.album-id >= 0`, swapping the grid for the detail.
    // Also stamps the Albums entry in `views.json`'s `last_detail_ids` so a
    // restart on the Albums tab reopens this same detail page. The grid's
    // cover cache is released off-thread once the fetch is in flight —
    // the grid view is unmounted by the `AlbumDetail.album-id` flip (see
    // `app-window.slint`'s `if` gate), so its covers are not visible and
    // not queried while detail is on screen; re-warmed by `on_close_detail`.
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        albums.on_open_album(move |album_id| {
            let id = i64::from(album_id);

            // Same-tab open: defensively zero any stale cross-section origin. A
            // "Go to Album" from Favorites stamps one and only `close-detail`
            // clears it, so reaching this grid by any path that left that detail
            // open would otherwise send the next back press to Favorites. The two
            // sibling grids carry the same line.
            if let Some(ui) = weak.upgrade() {
                ui.global::<AlbumDetail>().set_origin_nav_index(-1);
            }

            let s_fetch = s.clone();
            let au_fetch = au.clone();
            let weak_fetch = weak.clone();
            spawn_logged!(s_fetch, "albums::open_album",
                albums_ui_mod::open_album(
                    &s_fetch, &au_fetch, weak_fetch, id, crate::NavEnterFrom::Right));

            let au_release = au.clone();
            s.runtime.spawn_blocking(move || au_release.release_grid_covers());

            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_detail_id(
                    &s_disk,
                    crate::ui::track_list_view::view_id::ALBUM_DETAIL,
                    Some(id),
                ) {
                    log::warn!("albums::open_album persist: {e}");
                }
            });
        });
    }
}
