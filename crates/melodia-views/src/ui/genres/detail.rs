//! Genre Detail's own half: the header and hero fetch, open, refresh and the startup seed. Mirror
//! of `src/ui/albums/detail.rs` minus everything related to artwork: genres have no intrinsic
//! image. In its place [`apply_genre_hero`] hands the name-hashed colours to the backdrop, which
//! paints them as its gradient floor or washes them as an aurora depending on the arm.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::{GenresUi, genre_accent, to_slint_genre_row};
use crate::ui::appearance::rgb::color_to_rgb;
use crate::ui::detail_view::resolve_view_sort;
use crate::ui::hero_backdrop::GenreStops;
use crate::ui::hero_folds::HeroFold;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
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

/// Publish the genre's hero band from both of its hash-derived pairs — [`genre_accent`] picks them
/// off a name hash, and which one reaches the surface is the backdrop's to decide.
///
/// Hashed here rather than read back off the `GenreRow`: the row carries the saturated pair for the
/// square to paint, but the dimmed one has no Slint reader at all, so taking either from the row
/// means `slint::Color`s converted back into the numbers the hash produced — and a boundary struct
/// carrying two fields solely to hand them back to Rust.
///
/// `section_active` is the same gate `apply_detail_artwork` takes, and for the same reason:
/// `HeroBackdrop` is one global shared by six heroes, and the boot path seeds every persisted
/// detail id whichever section is being restored.
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

/// A genre's header and how far its tracks spread across artists and albums. Shared by
/// [`open_genre`] (fresh user open) and [`refresh_detail`] (watcher-driven refresh).
async fn fetch_genre_header(state: &AppState, genre_id: i64) -> AppResult<(GenreStats, HeroFold)> {
    let detail = library::genres::get_genre_detail(&state.db, genre_id).await?;
    let facts = library::keyset::facts(&state.db, TrackListScope::Genre(genre_id)).await?;
    Ok((detail, HeroFold::from(&facts)))
}

/// Fetch a genre's header + track list and populate the `GenreDetail` global — which flips
/// `genre-id >= 0`, swapping the grid for the detail view. Fresh-open semantics: resets the detail
/// sort to the album default and clears any prior selection. The watcher-driven refresh uses
/// [`refresh_detail`] instead, which preserves both.
pub async fn open_genre(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
    genre_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_genre_with(state, genres_ui, weak, genre_id, enter_from, |_ui| {}).await
}

/// [`open_genre`] plus a hook that runs inside the same UI-thread tick as the `genre-id` flip —
/// the `albums::detail::open_album_with` shape, and the cross-tab drill's only way to land on the
/// Genres tab **in the tick that opens the detail**. A second event-loop hop would put the tab
/// flip after the hero writes below, leaving them gated on a section not yet mounted.
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

    // Apply the persisted detail sort (one shared sort for every genre). `album` ascending is the
    // fresh-install default — genres span many albums, mirroring Artist Detail.
    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::GENRE_DETAIL, "album");
    let tracks = load::open(state, &**genres_ui, genre_id, &sort_field, &sort_dir).await?;

    genres_ui.detail.set_id(genre_id);

    let state = state.clone();
    let genres_ui = genres_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<GenreDetail>();
        let header = to_slint_genre_row(&detail);
        // The sort, an empty needle, no selection and the genre's tracks, so the user lands on the
        // full track set rather than a stale needle from the previous detail.
        tracks.seat(&state, &genres_ui);
        // The page's enter direction, set before the `on_applied` hook can
        // flip `Nav.selected-index`. Inert on a same-page drill, whose body
        // reads a fixed `below` — see `ui::nav_transition`.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_genre_id(clamp_i64_to_i32(genre_id));
        // Run after `genre-id` is set so the hook's own global writes land in the same UI-thread
        // tick as the detail flip — and *before* the two shared-hero writes below, which are gated
        // on the tab this hook moves to.
        on_applied(&ui);
        // The two globals six heroes share, written last because their gate is the live tab rather
        // than a shadow the `SectionActiveGate` only updates next frame: read before the hook
        // above, a cross-section drill answers for the tab the user *left*.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Genres);
        apply_genre_hero(&ui, &detail, on_screen);
        g.set_genre(header);
        crate::ui::hero_chips::publish_genre(&ui, &detail, fold, on_screen);
        crate::ui::nav_history::record_current(&ui);
        // Reseat the page's shared filter box, which the clear above doesn't reach — same
        // reasoning, and same closure position, as `albums::detail::open_album_with`.
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-read an already-open genre after a library change, **preserving** the user's current sort,
/// filter and selection. Same shape as `albums::detail::refresh_detail`.
pub async fn refresh_detail(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
    genre_id: i64,
) -> AppResult<()> {
    load::reload(state, genres_ui).await?;
    let (detail, fold) = fetch_genre_header(state, genre_id).await?;

    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<GenreDetail>();
        // The detail view was closed (or switched to another genre) while the fetch was in
        // flight — drop this stale refresh.
        if i64::from(g.get_genre_id()) != genre_id {
            return;
        }

        // Header is one row — always refresh it (counts / duration may have changed). Re-solving
        // the hero alongside it keeps this path identical to the open path.
        let header = to_slint_genre_row(&detail);
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Genres);
        apply_genre_hero(&ui, &detail, on_screen);
        g.set_genre(header);
        crate::ui::hero_chips::publish_genre(&ui, &detail, fold, on_screen);
    });
    Ok(())
}

/// Reopen the genre that was visible in the Genre Detail view at the last shutdown, if any: now if
/// the launch shows the Genres tab, otherwise on that tab's first enter. Called once at startup
/// *after* [`super::install`] so the `GenreDetail` callbacks are already live by the time
/// `open_genre`'s `upgrade_in_event_loop` lands. No-ops on a missing genre.
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
        // Above, not a drill-in slide: nobody navigated into this genre, it is their last view.
        if let Err(e) = open_genre(&s, &gu, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("genres::restore_detail open_genre({id}): {}", describe(&e));
        }
        // Lowered however it went, and behind `open_genre`'s own hop so the id is already in: a
        // genre gone since the last session owes the grid back rather than an empty body.
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<GenreDetail>().set_restoring(false);
        });
    });
}
