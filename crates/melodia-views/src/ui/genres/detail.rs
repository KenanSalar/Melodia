//! Genre Detail's own half: fetch, open, refresh and the startup seed. Mirror of
//! `src/ui/albums/detail.rs` minus everything related to artwork: genres have no intrinsic
//! image. In its place [`apply_genre_hero`] hands the name-hashed colours to the backdrop, which
//! paints them as its gradient floor or washes them as an aurora depending on the arm.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, SharedString, Weak};

use super::{GenresUi, genre_accent, to_slint_genre_row};
use crate::ui::appearance::theme_apply::color_to_rgb;
use crate::ui::detail_view::resolve_view_sort;
use crate::ui::hero_backdrop::GenreStops;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::track_detail::{DetailCache, TrackDetail, filter, selection};
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_list_rows;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::genre::GenreStats;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, GenreDetail, NavEnterFrom, TrackListRow as UiTrackListRow};

impl TrackDetail for GenresUi {
    type Global = GenreDetail<'static>;

    fn cache(&self) -> &DetailCache {
        &self.detail
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

/// Fetch a genre's header + track list and prewarm their cover thumbnails. Shared by
/// [`open_genre`] (fresh user open) and [`refresh_detail`] (watcher-driven refresh).
async fn fetch_genre_detail(
    state: &AppState,
    genres_ui: &GenresUi,
    genre_id: i64,
) -> AppResult<(GenreStats, Vec<RsTrackListRow>)> {
    let detail = library::genres::get_genre_detail(&state.db, genre_id).await?;
    let tracks = library::genres::get_genre_tracks(&state.db, genre_id).await?;

    // Prewarm the detail `TrackList`'s artwork column against the shared row-tier cache. Unlike
    // Albums / Artists Detail there is no separate header tile or hero blur.
    let track_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        tracks.iter().map(|t| t.artwork_path.as_deref()),
        genres_ui.cover_thumbs.capacity(),
    );
    if !track_covers.is_empty() {
        let row_thumbs = genres_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || {
            row_thumbs.prewarm(&track_covers);
        })
        .await;
    }
    Ok((detail, tracks))
}

/// Fetch a genre's header + track list, prewarm thumbnails, and populate the `GenreDetail` global
/// — which flips `genre-id >= 0`, swapping the grid for the detail view. Fresh-open semantics:
/// resets the detail sort to the album default and clears any prior selection. The watcher-driven
/// refresh uses [`refresh_detail`] instead, which preserves both.
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
    let (detail, mut tracks) = fetch_genre_detail(state, genres_ui, genre_id).await?;

    // Apply the persisted detail sort (one shared sort for every genre). `album` ascending is the
    // fresh-install default — genres span many albums, mirroring Artist Detail.
    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::GENRE_DETAIL, "album");
    sort_track_list_rows(&mut tracks, &sort_field, &sort_dir);

    // Build the `Send` half of every row here on the worker — only the `!Send` cover decode is
    // left for the UI thread, so the click→detail transition doesn't hitch on a large genre.
    let ui_tracks: Vec<UiTrackListRow> = crate::ui::tracks::to_slint_track_list_rows(&tracks);

    // How far the genre spreads — folded on the worker that fetched the rows,
    // since a broad genre's track list is the longest in the app.
    let fold = crate::ui::hero_folds::fold_tracks(&tracks);

    genres_ui.detail.set_id(genre_id);

    let genres_ui = genres_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<GenreDetail>();
        let header = to_slint_genre_row(&detail);
        filter::install_tracks(&g, ui_tracks);
        selection::reset(&g, &genres_ui.detail);
        // Fresh open clears the filter so the user lands on the full track set, not a stale
        // needle from the previous detail; `seat_unfiltered` below clears the Rust half.
        g.set_filter(SharedString::default());
        g.set_sort_field(SharedString::from(sort_field.as_str()));
        g.set_sort_dir(SharedString::from(sort_dir.as_str()));
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
        genres_ui.detail.seat_unfiltered(tracks);
        crate::ui::nav_history::record_current(&ui);
        // Reseat the page's shared filter box, which the clear above doesn't reach — same
        // reasoning, and same closure position, as `albums::detail::open_album_with`.
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-fetch an already-open genre's header + tracks after a library change, **preserving** the
/// user's current sort column and selection. Same shape as `albums::detail::refresh_detail`.
pub async fn refresh_detail(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
    genre_id: i64,
) -> AppResult<()> {
    let (detail, mut tracks) = fetch_genre_detail(state, genres_ui, genre_id).await?;

    let fold = crate::ui::hero_folds::fold_tracks(&tracks);

    let genres_ui = genres_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<GenreDetail>();
        // The detail view was closed (or switched to another genre) while the fetch was in
        // flight — drop this stale refresh.
        if i64::from(g.get_genre_id()) != genre_id {
            return;
        }

        // Apply the user's *current* sort to the freshly-fetched rows.
        let field = g.get_sort_field().to_string();
        let dir = g.get_sort_dir().to_string();
        sort_track_list_rows(&mut tracks, &field, &dir);

        // Header is one row — always refresh it (counts / duration may have changed). Re-solving
        // the hero alongside it keeps this path identical to the open path.
        let header = to_slint_genre_row(&detail);
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Genres);
        apply_genre_hero(&ui, &detail, on_screen);
        g.set_genre(header);
        crate::ui::hero_chips::publish_genre(&ui, &detail, fold, on_screen);

        // Prune `selected-ids` to ids that still exist, then let the shared filter pass
        // re-derive the displayed cache and the model from the canonical set. It diffs, so a
        // scan that touched unrelated files writes back only the rows whose content moved and
        // keeps the shift-range anchor.
        selection::prune_selection_to(&g, &tracks);
        genres_ui.detail.set_all_tracks(tracks);
        filter::apply_filtered_detail(&g, &genres_ui.detail);
    });
    Ok(())
}

/// Reopen the genre that was visible in the Genre Detail view at the last shutdown, if any. Called
/// once at startup *after* [`super::install`] so the `GenreDetail` callbacks are already live by
/// the time `open_genre`'s `upgrade_in_event_loop` lands. No-ops on a missing genre.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let Some(id) = library::settings::get_view_state(&state.paths).ok().and_then(|s| {
        s.last_detail_ids.get(crate::ui::track_list_view::view_id::GENRE_DETAIL).copied()
    }) else {
        return;
    };
    // Synchronously, so it is up before `app.show()` — see `AlbumDetail.restoring`.
    ui.global::<GenreDetail>().set_restoring(true);
    let s = state.clone();
    let gu = genres_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Above = first-launch fade-down, not a drill-in slide: the user didn't navigate, this is
        // restoring their last view.
        if let Err(e) = open_genre(&s, &gu, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("genres::seed_detail_from_settings open_genre({id}): {}", describe(&e));
        }
        // Lowered however it went, and behind `open_genre`'s own hop so the id is already in: a
        // genre gone since the last session owes the grid back rather than an empty body.
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<GenreDetail>().set_restoring(false);
        });
    });
}
