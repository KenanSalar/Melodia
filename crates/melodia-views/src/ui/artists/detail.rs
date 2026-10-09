//! Artist Detail's own half: fetch, artwork pair decode, open, refresh and the startup seed, plus
//! the Albums strip above the all-tracks `TrackList`. What the four details share is
//! [`crate::ui::track_detail`].

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use super::{ArtistsUi, to_slint_artist_row};
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_view::{apply_detail_artwork, resolve_view_sort};
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::track_detail::{DetailCache, TrackDetail, filter, selection};
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_list_rows;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{
    AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, NavEnterFrom, TrackListRow as UiTrackListRow,
};

impl TrackDetail for ArtistsUi {
    type Global = ArtistDetail<'static>;

    fn cache(&self) -> &DetailCache {
        &self.detail.cache
    }

    fn refilter(&self, g: &Self::Global) {
        apply_filtered_detail(g, self);
    }

    fn clear_detail(&self) {
        self.detail.cache.clear();
        self.detail.albums.lock().clear();
    }
}

/// Fetch an artist's header + albums sub-section + track list, and prewarm the row-tier covers for
/// the tracks. Shared by [`open_artist`] and [`refresh_detail`].
async fn fetch_artist_detail(
    state: &AppState,
    artists_ui: &ArtistsUi,
    artist_id: i64,
) -> AppResult<(ArtistStats, Vec<AlbumStats>, Vec<RsTrackListRow>)> {
    let detail = library::artists::get_artist_detail(&state.db, artist_id).await?;
    let albums = library::artists::get_artist_albums(&state.db, artist_id).await?;
    let tracks = library::artists::get_artist_tracks(&state.db, artist_id).await?;

    let track_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        tracks.iter().map(|t| t.artwork_path.as_deref()),
        artists_ui.cover_thumbs.capacity(),
    );
    // The Albums strip resolves its cards inline on the UI thread, its callback carrying no
    // generation to bring a scheduled cover back. Prewarm those covers alongside the track rows so
    // a detail open with a cold tier doesn't freeze the UI for one decode per album card.
    let strip_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        albums.iter().map(|a| a.artwork_path.as_deref()),
        crate::ui::grid_prewarm::tier().capacity(),
    );
    if !track_covers.is_empty() || !strip_covers.is_empty() {
        let row_thumbs = artists_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if !track_covers.is_empty() {
                row_thumbs.prewarm(&track_covers);
            }
            if !strip_covers.is_empty() {
                crate::ui::grid_prewarm::prewarm(&strip_covers);
            }
        })
        .await;
    }
    Ok((detail, albums, tracks))
}

/// Fetch an artist's header + albums + tracks, populate the `ArtistDetail` global, and flip
/// `artist-id >= 0` to swap the grid for the detail view. Fresh-open semantics: resets the detail
/// sort to the default and clears any prior selection.
pub async fn open_artist(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_artist_with(state, artists_ui, weak, artist_id, enter_from, |_ui| {}).await
}

/// Same as [`open_artist`] but the caller can hook into the **same** `upgrade_in_event_loop`
/// closure that writes `artist-id`. The hook runs after every detail property is set, so a
/// follow-on global write — flipping `Nav.selected-index` for a cross-tab drill — lands in the
/// same frame, and Slint paints `ArtistDetailBody` with no Artists-grid frame in between.
///
/// `enter_from` chooses the enter direction for the **page** mount a cross-section drill produces,
/// not for `ArtistDetailBody`, which takes a fixed `above` and holds still while the band morphs.
/// [`NavEnterFrom::Right`] for any user drill-in, [`NavEnterFrom::Above`] for the seed path.
pub async fn open_artist_with<F>(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
    enter_from: NavEnterFrom,
    on_applied: F,
) -> AppResult<()>
where
    F: FnOnce(&AppWindow) + Send + 'static,
{
    let (detail, albums, mut tracks) = fetch_artist_detail(state, artists_ui, artist_id).await?;

    // Apply the persisted detail sort on the worker so the per-row prep below runs in display
    // order, and the UI thread can drop already-sorted rows straight into the model. One shared
    // sort for every artist; `album` ascending is the fresh-install default.
    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::ARTIST_DETAIL, "album");
    sort_track_list_rows(&mut tracks, &sort_field, &sort_dir);

    let pair =
        decode_detail_pair(state, artists_ui.detail_artwork.clone(), detail.image_path.clone())
            .await;

    let ui_tracks: Vec<UiTrackListRow> = crate::ui::tracks::to_slint_track_list_rows(&tracks);

    // The album list is the artist's own discography, so its year span is what
    // "active 1957–1963" means here; folded on the worker that fetched it.
    let years = crate::ui::hero_folds::year_span(&albums);

    artists_ui.detail.cache.set_id(artist_id);

    let artists_ui = artists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<ArtistDetail>();

        let header = to_slint_artist_row(&detail);
        g.set_artist(header);

        // Albums sub-section model — small grid above the track list.
        let album_rows: Vec<UiAlbumRow> =
            albums.iter().map(crate::ui::albums::to_slint_album_row).collect();
        write_albums_model(&g, album_rows);

        filter::install_tracks(&g, ui_tracks);
        selection::reset(&g, &artists_ui.detail.cache);
        // Fresh open clears the filter so the user lands on the full tracks + albums set, not a
        // stale needle from the previous detail; `seat_unfiltered` below clears the Rust half.
        g.set_filter(SharedString::default());
        g.set_sort_field(SharedString::from(sort_field.as_str()));
        g.set_sort_dir(SharedString::from(sort_dir.as_str()));
        // Set the page's enter direction before the `on_applied` hook can flip
        // `Nav.selected-index`, so a cross-section drill's new page samples it on first paint.
        // Inert on a same-page drill, whose body reads a fixed `below`.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_artist_id(clamp_i64_to_i32(artist_id));
        artists_ui.detail.cache.seat_unfiltered(tracks);
        *artists_ui.detail.albums.lock() = albums;
        // Run after `artist-id` is set so any global writes the hook performs land in the same
        // UI-thread tick as the detail flip.
        on_applied(&ui);
        // The two globals six heroes share, written last because their gate is the **live** tab
        // rather than the `section_active` shadow — see `albums::detail::open_album_with`.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Artists);
        crate::ui::hero_chips::publish_artist(&ui, &detail, years, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        // Record a browser-style history entry — see `albums::detail::open_album_with`.
        crate::ui::nav_history::record_current(&ui);
        // Reseat the page's shared filter box, which the clear above doesn't reach — same
        // reasoning, and same closure position, as `albums::detail::open_album_with`.
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-fetch an already-open artist's header + albums + tracks after a library change, preserving
/// the user's current sort and selection.
pub async fn refresh_detail(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
) -> AppResult<()> {
    let (detail, albums, mut tracks) = fetch_artist_detail(state, artists_ui, artist_id).await?;

    let pair =
        decode_detail_pair(state, artists_ui.detail_artwork.clone(), detail.image_path.clone())
            .await;

    let years = crate::ui::hero_folds::year_span(&albums);

    let artists_ui = artists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<ArtistDetail>();
        if i64::from(g.get_artist_id()) != artist_id {
            return;
        }

        let field = g.get_sort_field().to_string();
        let dir = g.get_sort_dir().to_string();
        sort_track_list_rows(&mut tracks, &field, &dir);

        g.set_artist(to_slint_artist_row(&detail));
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Artists);
        crate::ui::hero_chips::publish_artist(&ui, &detail, years, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, on_screen);

        selection::prune_selection_to(&g, &tracks);
        // The canonical sets only: the filter pass rewrites the displayed cache and both models,
        // so a live needle survives the refresh with no flash of unfiltered rows.
        artists_ui.detail.cache.set_all_tracks(tracks);
        *artists_ui.detail.albums.lock() = albums;
        apply_filtered_detail(&g, &artists_ui);
    });
    Ok(())
}

/// The shared filter pass, then the Albums strip, which only this detail has and which matches
/// on album name alone. UI thread.
fn apply_filtered_detail(g: &ArtistDetail<'_>, artists_ui: &ArtistsUi) {
    filter::apply_filtered_detail(g, &artists_ui.detail.cache);

    let needle = artists_ui.detail.cache.needle();
    let album_rows: Vec<UiAlbumRow> = artists_ui
        .detail
        .albums
        .lock()
        .iter()
        .filter(|a| needle.contains(&a.name))
        .map(crate::ui::albums::to_slint_album_row)
        .collect();
    write_albums_model(g, album_rows);
}

/// Reopen the artist that was visible at the last shutdown, if any.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    let Some(id) = library::settings::get_view_state(&state.paths).ok().and_then(|s| {
        s.last_detail_ids.get(crate::ui::track_list_view::view_id::ARTIST_DETAIL).copied()
    }) else {
        return;
    };
    // Synchronously, so it is up before `app.show()` — see `AlbumDetail.restoring`.
    ui.global::<ArtistDetail>().set_restoring(true);
    artists_ui.section.begin_restore();
    let s = state.clone();
    let au = artists_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Above = first-launch fade-down, not a drill-in slide: the user didn't navigate, this is
        // restoring their last view.
        if let Err(e) = open_artist(&s, &au, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("artists::seed_detail_from_settings open_artist({id}): {}", describe(&e));
        }
        // Lowered however it went, and behind `open_artist`'s own hop so the id is already in: an
        // artist gone since the last session owes the grid back rather than an empty body.
        au.section.end_restore();
        // A grid handed back is warmed before the hop below shows it; a reopen turns this away.
        crate::ui::grid_prewarm::prewarm_off_thread(&au, ArtistsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<ArtistDetail>().set_restoring(false);
        });
    });
}

/// Swap the `ArtistDetail.albums` `VecModel` contents in place, through the keyed diff so a
/// filter keystroke that leaves the strip alone patches rather than rebuilding every delegate.
/// `filter::install_tracks`' shape, one model over.
fn write_albums_model(g: &ArtistDetail, rows: Vec<UiAlbumRow>) {
    let model = g.get_albums();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiAlbumRow>>() {
        crate::ui::model_diff::apply_rows_keyed(vm, rows, |r| r.id);
    } else {
        g.set_albums(ModelRc::from(Rc::new(VecModel::from(rows))));
    }
}
