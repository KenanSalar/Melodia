//! Genre Detail's own half: the header and hero fetch, open, refresh and the startup seed. Album
//! Detail's shape less the artwork, [`apply_genre_hero`] handing the backdrop name-hashed colours
//! in its place.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::{GenresUi, genre_accent, to_slint_genre_row};
use crate::ui::appearance::rgb::color_to_rgb;
use crate::ui::detail_view::resolve_view_sort;
use crate::ui::hero_backdrop::GenreStops;
use crate::ui::hero_folds::HeroFold;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::paged_rows::track_rows;
use crate::ui::section_state::SectionState;
use crate::ui::track_detail::{DetailCache, TrackDetail, load, restore};
use crate::ui::track_list_view::view_id;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::genre::GenreStats;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, GenreDetail, NavEnterFrom};

impl TrackDetail for GenresUi {
    type Global = GenreDetail<'static>;

    const TAB: MyLibraryTab = MyLibraryTab::Genres;

    fn cache(&self) -> &DetailCache<Self::Global> {
        &self.detail
    }

    fn section(&self) -> &SectionState {
        &self.section
    }

    fn row_covers(&self) -> &Arc<CoverThumbs> {
        &self.cover_thumbs
    }

    fn scope(&self, id: i64) -> TrackListScope {
        TrackListScope::Genre(id)
    }
}

/// Publishes the hero band from both of [`genre_accent`]'s pairs, the backdrop deciding which one
/// reaches the surface. Hashed again rather than read off the `GenreRow`, which carries only the
/// pair a Slint reader paints.
///
/// `section_active` is `apply_detail_artwork`'s gate, for its reason.
fn apply_genre_hero(ui: &AppWindow, genre: &GenreStats, section_active: bool) {
    if !section_active {
        return;
    }
    let accent = genre_accent(&genre.name);
    crate::ui::hero_backdrop::apply_gradient(
        ui,
        GenreStops {
            floor: (color_to_rgb(accent.hero_color_1), color_to_rgb(accent.hero_color_2)),
            wash: (color_to_rgb(accent.tile_color_1), color_to_rgb(accent.tile_color_2)),
        },
    );
}

/// A genre's header and how far its tracks spread across artists and albums.
async fn fetch_genre_header(state: &AppState, genre_id: i64) -> AppResult<(GenreStats, HeroFold)> {
    let detail = library::genres::get_genre_detail(&state.db, genre_id).await?;
    let facts = library::keyset::facts(&state.db, TrackListScope::Genre(genre_id)).await?;
    Ok((detail, HeroFold::from(&facts)))
}

/// Fetches a genre's header and tracks into `GenreDetail`, as
/// [`crate::ui::albums::open_album_with`] does an album's.
pub async fn open_genre(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
    genre_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_genre_with(state, genres_ui, weak, genre_id, enter_from, |_ui| {}).await
}

/// [`open_genre`] with a hook, on [`crate::ui::albums::open_album_with`]'s contract.
pub async fn open_genre_with<F>(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
    genre_id: i64,
    enter_from: NavEnterFrom,
    on_applied: F,
) -> AppResult<()>
where
    F: FnOnce(&AppWindow) + Send + 'static,
{
    let (detail, fold) = fetch_genre_header(state, genre_id).await?;

    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::GENRE_DETAIL, "album");
    let tracks = load::open(state, &**genres_ui, genre_id, &sort_field, &sort_dir).await?;

    genres_ui.detail.set_id(genre_id);

    let state = state.clone();
    let genres_ui = genres_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<GenreDetail>();
        let header = to_slint_genre_row(&detail);
        tracks.seat(&state, &genres_ui);
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_genre_id(clamp_i64_to_i32(genre_id));
        on_applied(&ui);
        // From here on, in `open_album_with`'s order and for its reasons.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Genres);
        apply_genre_hero(&ui, &detail, on_screen);
        g.set_genre(header);
        crate::ui::hero_chips::publish_genre(&ui, &detail, fold, on_screen);
        crate::ui::nav_history::record_current(&ui);
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-reads an open genre after a library change as `albums::refresh_detail` does.
pub async fn refresh_detail(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
    genre_id: i64,
) -> AppResult<()> {
    track_rows::reload(state, genres_ui).await?;
    let (detail, fold) = fetch_genre_header(state, genre_id).await?;

    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<GenreDetail>();
        if i64::from(g.get_genre_id()) != genre_id {
            return;
        }

        let header = to_slint_genre_row(&detail);
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Genres);
        apply_genre_hero(&ui, &detail, on_screen);
        g.set_genre(header);
        crate::ui::hero_chips::publish_genre(&ui, &detail, fold, on_screen);
    });
    Ok(())
}

/// Reopens the genre on screen at the last shutdown: now if the launch shows the Genres tab,
/// otherwise on that tab's first enter.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    restore::seed_or_defer(ui, state, genres_ui, restore_detail);
}

/// Reopen a persisted genre, at launch or on the first enter of a tab the launch left hidden.
pub fn restore_detail(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>, id: i64) {
    // Synchronously, so it is up before the tab's first frame; see `AlbumDetail.restoring`.
    ui.global::<GenreDetail>().set_restoring(true);
    let s = state.clone();
    let gu = genres_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // In `albums::restore_detail`'s order and for its reasons.
        if let Err(e) = open_genre(&s, &gu, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("genres::restore_detail open_genre({id}): {}", describe(&e));
        }
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<GenreDetail>().set_restoring(false);
        });
    });
}
