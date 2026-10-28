//! Artist Detail's own half: the header and Albums strip fetch, the artwork pair decode, open,
//! refresh and the startup seed. What the four details share is [`crate::ui::track_detail`].

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::{ArtistsUi, to_slint_artist_row};
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_view::{apply_detail_artwork, resolve_view_sort};
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::paged_rows::track_rows;
use crate::ui::section_state::SectionState;
use crate::ui::track_detail::{DetailCache, TrackDetail, load, restore};
use crate::ui::track_list_view::view_id;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, NavEnterFrom};

impl TrackDetail for ArtistsUi {
    type Global = ArtistDetail<'static>;

    const TAB: MyLibraryTab = MyLibraryTab::Artists;

    fn cache(&self) -> &DetailCache<Self::Global> {
        &self.detail.cache
    }

    fn section(&self) -> &SectionState {
        &self.section
    }

    fn row_covers(&self) -> &Arc<CoverThumbs> {
        &self.cover_thumbs
    }

    fn scope(&self, id: i64) -> TrackListScope {
        TrackListScope::Artist(id)
    }

    /// The Albums strip, which only this detail has and which matches on album name alone.
    fn refilter_beside(&self, g: &Self::Global) {
        apply_albums_filter(g, self);
    }

    fn clear_detail(&self) {
        self.detail.cache.clear();
        self.detail.albums.lock().clear();
    }
}

/// An artist's header and Albums strip, with the strip's covers prewarmed.
async fn fetch_artist_header(
    state: &AppState,
    artist_id: i64,
) -> AppResult<(ArtistStats, Vec<AlbumStats>)> {
    let detail = library::artists::get_artist_detail(&state.db, artist_id).await?;
    let albums = library::artists::get_artist_albums(&state.db, artist_id).await?;

    // The strip decodes its covers inline on the UI thread, its callback carrying no generation to
    // bring a scheduled one back, so a cold tier would stall one decode per card.
    let strip_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        albums.iter().map(|a| a.artwork_path.as_deref()),
        crate::ui::grid_prewarm::tier().capacity(),
    );
    if !strip_covers.is_empty() {
        let _ = tokio::task::spawn_blocking(move || {
            crate::ui::grid_prewarm::prewarm(&strip_covers);
        })
        .await;
    }
    Ok((detail, albums))
}

/// Fetches an artist's header, albums and tracks into `ArtistDetail`, as
/// [`crate::ui::albums::open_album_with`] does an album's.
pub async fn open_artist(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_artist_with(state, artists_ui, weak, artist_id, enter_from, |_ui| {}).await
}

/// [`open_artist`] with a hook, on [`crate::ui::albums::open_album_with`]'s contract.
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
    let (detail, albums) = fetch_artist_header(state, artist_id).await?;

    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::ARTIST_DETAIL, "album");
    let tracks = load::open(state, &**artists_ui, artist_id, &sort_field, &sort_dir).await?;

    let pair =
        decode_detail_pair(state, artists_ui.detail_artwork.clone(), detail.image_path.clone())
            .await;

    // The artist's own discography, so its year span is what "active 1957–1963" means here.
    let years = crate::ui::hero_folds::year_span(&albums);

    artists_ui.detail.cache.set_id(artist_id);

    let state = state.clone();
    let artists_ui = artists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<ArtistDetail>();

        let header = to_slint_artist_row(&detail);
        g.set_artist(header);

        let album_rows: Vec<UiAlbumRow> =
            albums.iter().map(crate::ui::albums::to_slint_album_row).collect();
        write_albums_model(&g, album_rows);
        *artists_ui.detail.albums.lock() = albums;

        tracks.seat(&state, &artists_ui);
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_artist_id(clamp_i64_to_i32(artist_id));
        on_applied(&ui);
        // From here on, in `open_album_with`'s order and for its reasons.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Artists);
        crate::ui::hero_chips::publish_artist(&ui, &detail, years, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        crate::ui::nav_history::record_current(&ui);
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-reads an open artist after a library change as `albums::refresh_detail` does, the strip
/// included.
pub async fn refresh_detail(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
) -> AppResult<()> {
    track_rows::reload(state, artists_ui).await?;
    let (detail, albums) = fetch_artist_header(state, artist_id).await?;

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

        g.set_artist(to_slint_artist_row(&detail));
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Artists);
        crate::ui::hero_chips::publish_artist(&ui, &detail, years, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, on_screen);

        // The canonical strip only: the filter pass narrows it, so a live needle survives the
        // refresh with no flash of unfiltered cards.
        *artists_ui.detail.albums.lock() = albums;
        apply_albums_filter(&g, &artists_ui);
    });
    Ok(())
}

/// Narrows the Albums strip to the cards whose name the needle matches. UI thread.
fn apply_albums_filter(g: &ArtistDetail<'_>, artists_ui: &ArtistsUi) {
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

/// Reopens the artist on screen at the last shutdown: now if the launch shows the Artists tab,
/// otherwise on that tab's first enter.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    restore::seed_or_defer(ui, state, artists_ui, restore_detail);
}

/// Reopen a persisted artist, at launch or on the first enter of a tab the launch left hidden.
pub fn restore_detail(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>, id: i64) {
    // Synchronously, so it is up before the tab's first frame; see `AlbumDetail.restoring`.
    ui.global::<ArtistDetail>().set_restoring(true);
    artists_ui.section.begin_restore();
    let s = state.clone();
    let au = artists_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // In `albums::restore_detail`'s order and for its reasons.
        if let Err(e) = open_artist(&s, &au, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("artists::restore_detail open_artist({id}): {}", describe(&e));
        }
        au.section.end_restore();
        crate::ui::grid_prewarm::prewarm_off_thread(&au, ArtistsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<ArtistDetail>().set_restoring(false);
        });
    });
}

/// Writes the strip through the keyed diff, so a keystroke that leaves it alone rebuilds no card.
fn write_albums_model(g: &ArtistDetail, rows: Vec<UiAlbumRow>) {
    let model = g.get_albums();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiAlbumRow>>() {
        crate::ui::model_diff::apply_rows_keyed(vm, rows, |r| r.id);
    } else {
        g.set_albums(ModelRc::from(Rc::new(VecModel::from(rows))));
    }
}
