//! Genre Detail header + track list: fetch, re-sort, refresh-preserving, startup seed. Mirror of
//! `src/ui/albums/detail.rs` minus everything related to artwork (`decode_detail_pair`,
//! `apply_detail_artwork`, `write_crossfade_slot`): genres have no intrinsic image. In its place
//! [`apply_genre_hero`] publishes the name-hashed gradient as the hero's backdrop and solves the
//! rest of the band against it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::selection::{apply_selection_to_rows, write_selection};
use super::{GenresUi, to_slint_genre_row};
use crate::entities::genre::GenreStats;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::themes::color_to_rgb;
use crate::ui::detail_filter::FilterRefs;
use crate::ui::detail_view::{impl_detail_view_helpers, resolve_view_sort};
use crate::ui::model_patch;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_list_rows;
use crate::ui::tracks::PreparedTrackRow;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AppWindow, GenreDetail, GenreRow as UiGenreRow, NavEnterFrom, TrackListRow as UiTrackListRow,
};

/// Publish the genre's hero band: its two hash-derived stops become the gradient floor verbatim,
/// and the scrim + foreground tiers are solved against how bright that gradient measures.
///
/// The stops are already theme-independent (`super::color::genre_accent` picks them off a name
/// hash and deliberately dims the hero pair), so unlike every other hero this one keeps its own
/// floor rather than taking the solved one.
///
/// `section_active` is the same gate `apply_detail_artwork` takes, and for the same reason:
/// `HeroBackdrop` is one global shared by six heroes, and the boot path seeds every persisted
/// detail id whichever section is being restored.
fn apply_genre_hero(ui: &AppWindow, header: &UiGenreRow, section_active: bool) {
    if !section_active {
        return;
    }
    crate::ui::hero_backdrop::apply_gradient(
        ui,
        color_to_rgb(header.hero_color_1),
        color_to_rgb(header.hero_color_2),
    );
}

/// Fetch a genre's header + track list and prewarm their cover thumbnails. Shared by
/// [`open_genre`] (fresh user open) and [`refresh_detail`] (watcher-driven refresh).
async fn fetch_genre_detail(
    state: &AppState,
    genres_ui: &GenresUi,
    genre_id: i64,
) -> AppResult<(GenreStats, Vec<RsTrackListRow>)> {
    let detail = library::genres::get_genre_detail(state, genre_id).await?;
    let tracks = library::genres::get_genre_tracks(state, genre_id).await?;

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
    let prepared: Vec<PreparedTrackRow> =
        tracks.iter().map(crate::ui::tracks::prepare_track_list_row).collect();

    // How far the genre spreads — folded on the worker that fetched the rows,
    // since a broad genre's track list is the longest in the app.
    let fold = crate::ui::hero_folds::fold_tracks(&tracks);

    *genres_ui.detail.genre_id.lock() = genre_id;

    let genres_ui = genres_ui.clone();
    let state_for_history = state.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<GenreDetail>();
        // UI-thread step: just the cover lookups + the model swap.
        let ui_tracks: Vec<UiTrackListRow> =
            prepared.into_iter().map(crate::ui::tracks::finish_track_list_row).collect();
        let header = to_slint_genre_row(&detail);
        replace_tracks_model(&g, ui_tracks);
        reset_detail_selection(&g, &genres_ui);
        // Fresh open clears the filter so the user lands on the full
        // track set, not a stale needle from the previous detail.
        g.set_filter(SharedString::from(""));
        genres_ui.detail.filter.lock().clear();
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
        apply_genre_hero(&ui, &header, on_screen);
        g.set_genre(header);
        crate::ui::hero_chips::publish_genre(&ui, &detail, fold, on_screen);
        // Fresh open: no filter, so the displayed cache equals the canonical full set.
        genres_ui.detail.all_tracks.lock().clone_from(&tracks);
        *genres_ui.detail.tracks.lock() = tracks;
        crate::ui::nav_history::record_current(&state_for_history, &ui);
        // Reseat the page's shared filter box, which the clear above doesn't reach — same
        // reasoning, and same closure position, as `albums::detail::open_album_with`.
        ui.global::<crate::MyLibrary>().invoke_detail_scope_changed();
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
        apply_genre_hero(&ui, &header, on_screen);
        g.set_genre(header);
        crate::ui::hero_chips::publish_genre(&ui, &detail, fold, on_screen);

        // With an active filter the displayed model is a subset, so the id-slice fast path below
        // (which assumes an unfiltered model) would drop the needle. Route the swap through
        // `apply_filtered_detail` instead so the filter survives.
        if !genres_ui.detail.filter.lock().is_empty() {
            let valid: std::collections::HashSet<i32> =
                tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect();
            let pruned: Vec<i32> =
                g.get_selected_ids().iter().filter(|id| valid.contains(id)).collect();
            write_selection(&g, pruned);
            // Refresh the canonical full set; `apply_filtered_detail` re-derives the displayed
            // `tracks` cache + model from it.
            *genres_ui.detail.all_tracks.lock() = tracks;
            apply_filtered_detail(&ui, &genres_ui);
            return;
        }

        // Take the cheap in-place path when the visible id slice is unchanged — the common case
        // when a scan touched unrelated files, or edited a track already on screen.
        let new_ids: Vec<i32> = tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect();
        let cur_ids: Vec<i32> = {
            let model = g.get_tracks();
            model
                .as_any()
                .downcast_ref::<VecModel<UiTrackListRow>>()
                .map(|vm| {
                    (0..vm.row_count()).filter_map(|i| vm.row_data(i)).map(|r| r.id).collect()
                })
                .unwrap_or_default()
        };
        if new_ids == cur_ids {
            // Id slice + order unchanged: surgical row updates only.
            let model = g.get_tracks();
            if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
                for (i, t) in tracks.iter().enumerate() {
                    let Some(old) = vm.row_data(i) else { continue };
                    let mut fresh = crate::ui::tracks::to_slint_track_list_row(t);
                    // Selection is unchanged on this branch, so keep it.
                    fresh.selected = old.selected;
                    if fresh != old {
                        vm.set_row_data(i, fresh);
                    }
                }
            }
            // No filter on this path — displayed cache equals canonical.
            genres_ui.detail.all_tracks.lock().clone_from(&tracks);
            *genres_ui.detail.tracks.lock() = tracks;
        } else {
            let ui_tracks: Vec<UiTrackListRow> =
                tracks.iter().map(crate::ui::tracks::to_slint_track_list_row).collect();
            replace_tracks_model(&g, ui_tracks);
            // The fresh rows all carry `selected: false`, so the `applied` shadow must be reset to
            // match *before* re-applying — and the Rust-side cache must already hold the new
            // (sorted) tracks so `apply_selection_to_rows` can resolve ids → row indices.
            genres_ui.detail.applied_selection.lock().clear();
            // No filter on this path — displayed cache equals canonical.
            genres_ui.detail.all_tracks.lock().clone_from(&tracks);
            *genres_ui.detail.tracks.lock() = tracks;
            // Indices shifted — prune the selection to surviving ids and reset the anchor, then
            // re-apply to the fresh rows.
            let valid: std::collections::HashSet<i32> = new_ids.iter().copied().collect();
            let pruned: Vec<i32> =
                g.get_selected_ids().iter().filter(|id| valid.contains(id)).collect();
            write_selection(&g, pruned);
            g.set_selection_anchor(-1);
            apply_selection_to_rows(&g, &genres_ui);
        }
    });
    Ok(())
}

/// Re-sort the cached detail tracks to the current `GenreDetail` sort state, then reorder the
/// existing `tracks` model rows to match. No DB hit, **and no row rebuild**. Runs on the UI
/// thread. Selection is preserved — track ids are stable across a re-sort.
pub fn resort_detail(ui: &AppWindow, genres_ui: &GenresUi) {
    let g = ui.global::<GenreDetail>();
    let field = g.get_sort_field().to_string();
    let dir = g.get_sort_dir().to_string();

    // Sort the Rust caches first — `play-row` / range-select read the displayed `tracks`;
    // `all_tracks` is sorted in lockstep so widening the filter later still yields sorted rows.
    let order: Vec<i32> = {
        sort_track_list_rows(&mut genres_ui.detail.all_tracks.lock(), &field, &dir);
        let mut tracks = genres_ui.detail.tracks.lock();
        sort_track_list_rows(&mut tracks, &field, &dir);
        tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect()
    };

    // Reorder the existing Slint rows to match. Pulling each row out of the model and re-emitting
    // it in the new order just moves the refcounted struct — no decode, no `format!`.
    let model = g.get_tracks();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        let mut by_id: HashMap<i32, UiTrackListRow> = HashMap::with_capacity(vm.row_count());
        for i in 0..vm.row_count() {
            if let Some(r) = vm.row_data(i) {
                by_id.insert(r.id, r);
            }
        }
        let reordered: Vec<UiTrackListRow> =
            order.iter().filter_map(|id| by_id.remove(id)).collect();
        vm.set_vec(reordered);
    }
    // Reordered structs keep their `selected` flags — defensive re-sync.
    apply_selection_to_rows(&g, genres_ui);
}

/// Clear the detail view's cached state when the user navigates back to the grid. The Slint side
/// has already flipped `genre-id` to `-1`.
pub fn clear_detail(genres_ui: &GenresUi) {
    *genres_ui.detail.genre_id.lock() = -1;
    genres_ui.detail.tracks.lock().clear();
    genres_ui.detail.all_tracks.lock().clear();
    genres_ui.detail.applied_selection.lock().clear();
    genres_ui.detail.filter.lock().clear();
}

/// Update the cached filter needle. The Slint side already mirrors the live text via the `<=>`
/// binding; this Rust mirror lets `refresh_detail` re-apply the filter to fresh data without
/// round-tripping the UI thread for the property read. Always stored folded so the per-keystroke
/// walk doesn't re-fold per row.
pub fn set_filter(genres_ui: &GenresUi, needle: &str) {
    *genres_ui.detail.filter.lock() = crate::ui::row_match::fold_needle(needle);
}

/// Re-walk the cached tracks through the current filter and push the filtered Slint model — see
/// [`crate::ui::detail_filter`] for the shared implementation. Runs on the UI thread.
pub fn apply_filtered_detail(ui: &AppWindow, genres_ui: &GenresUi) {
    let g = ui.global::<GenreDetail>();
    crate::ui::detail_filter::apply_filtered_detail(
        &g,
        &FilterRefs {
            all_tracks: &genres_ui.detail.all_tracks,
            tracks: &genres_ui.detail.tracks,
            applied: &genres_ui.detail.applied_selection,
            filter: &genres_ui.detail.filter,
        },
    );
}

/// Flip `is_favorite` on a single detail row in the Slint `VecModel`. Only touches the affected
/// row — scroll position and neighbours stay put. Mirrors `albums::apply_detail_row_favorite`.
pub fn apply_detail_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<GenreDetail>().get_tracks(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// Set `rating` on a single detail row in the Slint `VecModel`. Mirrors
/// [`apply_detail_row_favorite`].
pub fn apply_detail_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<GenreDetail>().get_tracks(), id, |r| {
            r.rating = rating;
        });
    });
}

/// Reopen the genre that was visible in the Genre Detail view at the last shutdown, if any. Called
/// once at startup *after* [`super::install`] so the `GenreDetail` callbacks are already live by
/// the time `open_genre`'s `upgrade_in_event_loop` lands. No-ops on a missing genre.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, genres_ui: &Arc<GenresUi>) {
    let Some(id) = library::settings::get_view_state(state).ok().and_then(|s| {
        s.last_detail_ids.get(crate::ui::track_list_view::view_id::GENRE_DETAIL).copied()
    }) else {
        return;
    };
    let s = state.clone();
    let gu = genres_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Below = first-launch fade-up, not a drill-in slide: the user didn't navigate, this is
        // restoring their last view.
        if let Err(e) = open_genre(&s, &gu, weak, id, NavEnterFrom::Below).await {
            log::warn!("genres::seed_detail_from_settings open_genre({id}): {e}");
        }
    });
}

/// Clear the `selected-ids` model + anchor without walking the row
/// model — freshly-built rows already carry `selected: false`, so the
/// `applied` shadow is reset to empty to match.
pub(super) fn reset_detail_selection(g: &GenreDetail, genres_ui: &GenresUi) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(-1);
    genres_ui.detail.applied_selection.lock().clear();
}

// `replace_tracks_model` — in-place `tracks` `VecModel` swap. Genres
// have no header artwork, so the `no_artwork` arm omits
// `apply_detail_artwork`. See `src/ui/detail_view.rs`.
impl_detail_view_helpers!(no_artwork GenreDetail);
