//! Album Detail header + track list: fetch, artwork pair decode, re-sort,
//! refresh-preserving, startup seed.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, SharedString, Weak};

use super::selection::{apply_selection_to_rows, write_selection};
use super::{AlbumsUi, to_slint_album_row};
use crate::entities::album::AlbumStats;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_filter::FilterRefs;
use crate::ui::detail_selection::prune_selection_to;
use crate::ui::detail_view::{impl_detail_view_helpers, resolve_view_sort};
use crate::ui::model_patch;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_list_rows;
use crate::ui::tracks::PreparedTrackRow;
use crate::ui::util::clamp_i64_to_i32;
use crate::{AlbumDetail, AppWindow, NavEnterFrom, TrackListRow as UiTrackListRow};

// `apply_detail_artwork` (cover + hero-blur write) and
// `replace_tracks_model` (in-place `tracks` `VecModel` swap) — see
// `src/ui/detail_view.rs`.
impl_detail_view_helpers!(artwork AlbumDetail);

/// Fetch an album's header + track list and prewarm their cover
/// thumbnails. Shared by [`open_album`] (fresh user open) and
/// [`refresh_detail`] (watcher-driven refresh).
async fn fetch_album_detail(
    state: &AppState,
    albums_ui: &AlbumsUi,
    album_id: i64,
) -> AppResult<(AlbumStats, Vec<RsTrackListRow>)> {
    let detail = library::albums::get_album_detail(state, album_id).await?;
    let tracks = library::albums::get_album_tracks(state, album_id).await?;

    // The `TrackList`'s artwork column, against the shared row tier. The header
    // tile and hero blur go through `decode_detail_pair` instead.
    let track_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        tracks.iter().map(|t| t.artwork_path.as_deref()),
        albums_ui.cover_thumbs.capacity(),
    );
    if !track_covers.is_empty() {
        let row_thumbs = albums_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || {
            row_thumbs.prewarm(&track_covers);
        })
        .await;
    }
    Ok((detail, tracks))
}

/// Fetch an album's header and track list and populate the `AlbumDetail` global,
/// whose `album-id >= 0` swaps the grid for the detail view. Fresh-open
/// semantics: resets the sort to the track-number default and clears any
/// selection, where the watcher-driven [`refresh_detail`] preserves both.
pub async fn open_album(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_album_with(state, albums_ui, weak, album_id, enter_from, |_ui| {}).await
}

/// [`open_album`] with a hook into the **same** `upgrade_in_event_loop` closure
/// that writes `album-id`, running after every detail property is set — so a
/// follow-on global write (`Nav.selected-index` for a cross-tab drill) lands in
/// the same frame and Slint paints `AlbumDetailBody` with no grid frame in
/// between. What follows the hook is the handful of writes that must read the
/// tab it may have just moved: the shared-hero gate, the history record and the
/// filter-box reseat.
///
/// `enter_from` is the enter direction of the **page** mount a cross-section
/// drill produces; `AlbumDetailBody` takes a fixed `below` and holds still while
/// the band morphs, so it reaches nothing when `Nav.selected-index` doesn't move
/// in the same tick.
pub async fn open_album_with<F>(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
    enter_from: NavEnterFrom,
    on_applied: F,
) -> AppResult<()>
where
    F: FnOnce(&AppWindow) + Send + 'static,
{
    let (detail, mut tracks) = fetch_album_detail(state, albums_ui, album_id).await?;

    // One shared sort for every album, restored across opens and restarts.
    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::ALBUM_DETAIL, "track_number");
    sort_track_list_rows(&mut tracks, &sort_field, &sort_dir);

    // Both halves come off one source decode. The buffers stay raw RGB8 so they
    // can cross the `upgrade_in_event_loop` boundary below.
    let pair =
        decode_detail_pair(state, albums_ui.detail_artwork.clone(), detail.artwork_path.clone())
            .await;

    // The `Send` half of every row, built on the worker so only the `!Send`
    // cover lookup is left for the UI thread — otherwise the click→detail
    // transition hitches on a long album.
    let prepared: Vec<PreparedTrackRow> =
        tracks.iter().map(crate::ui::tracks::prepare_track_list_row).collect();

    // Folded here rather than in the closure below: this is the worker that
    // already holds the rows.
    let genre = crate::ui::hero_folds::dominant_genre(&tracks);

    *albums_ui.detail.album_id.lock() = album_id;

    let albums_ui = albums_ui.clone();
    let state_for_history = state.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<AlbumDetail>();
        let ui_tracks: Vec<UiTrackListRow> =
            prepared.into_iter().map(crate::ui::tracks::finish_track_list_row).collect();
        let header = to_slint_album_row(&detail);
        g.set_album(header);
        replace_tracks_model(&g, ui_tracks);
        reset_detail_selection(&g, &albums_ui);
        // A fresh open lands on the full track set, not the previous detail's
        // needle. Slint property and Rust cache cleared together.
        g.set_filter(SharedString::from(""));
        albums_ui.detail.filter.lock().clear();
        g.set_sort_field(SharedString::from(sort_field.as_str()));
        g.set_sort_dir(SharedString::from(sort_dir.as_str()));
        // Marked before the hook can flip `Nav.selected-index`, so a
        // cross-section drill's new page samples it on first paint. Inert on a
        // same-page drill, whose body reads a fixed `below`.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_album_id(clamp_i64_to_i32(album_id));
        // No filter yet, so the displayed cache equals the canonical set.
        albums_ui.detail.all_tracks.lock().clone_from(&tracks);
        *albums_ui.detail.tracks.lock() = tracks;
        // After `album-id`, so whatever globals the hook writes land in the same
        // UI-thread tick as the detail flip.
        on_applied(&ui);
        // The two globals six heroes share, written last because their gate is
        // the **live** tab rather than the `section_active` shadow the
        // `SectionActiveGate` only updates next frame. Read before the hook, a
        // cross-section drill answers for the tab the user *left*.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Albums);
        // Off `detail` rather than the `AlbumRow` above, which carries neither
        // `disc_count` nor `is_compilation`.
        crate::ui::hero_chips::publish_album(&ui, &detail, genre.as_deref(), on_screen);
        // The blur cross-fades; the cover slot is written directly, the artwork
        // tile being covered by the next album's in one frame.
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        // After the hook, so a cross-tab drill records the post-flip section.
        // No-op while a Mouse-4/5 replay is in flight.
        crate::ui::nav_history::record_current(&state_for_history, &ui);
        // The filter clear above is only half a clear: the page's one box is
        // `MyLibrary.filter`, and the sheet's `album-id` mirror can't announce a
        // re-open writing the *same* id — which a section re-enter over an open
        // detail is, leaving the box holding a needle this call just cleared.
        // Last in the closure because `sync_box` reads the mounted tab, which
        // `on_applied` may have just moved.
        ui.global::<crate::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-fetch an open album after a library change, **preserving** the sort column, the filter
/// and the selection: the library-changed subscriber fires on every watcher and scan tick, so
/// it must not silently reset any of them. The shared filter pass diffs, so a scan that touched
/// unrelated files writes back only the rows whose content moved and leaves the delegate cache
/// standing. Bails if the detail was closed or navigated away while the fetch was in flight.
pub async fn refresh_detail(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
) -> AppResult<()> {
    let (detail, mut tracks) = fetch_album_detail(state, albums_ui, album_id).await?;

    // Re-decode the `(cover, blur)` pair — the artwork may have changed
    // (cover swap, replace-on-disk). `get_or_decode` is a cache hit when
    // the path is unchanged, so this is cheap in the steady state.
    let pair =
        decode_detail_pair(state, albums_ui.detail_artwork.clone(), detail.artwork_path.clone())
            .await;

    let genre = crate::ui::hero_folds::dominant_genre(&tracks);

    let albums_ui = albums_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<AlbumDetail>();
        // The detail view was closed (or switched to another album) while
        // the fetch was in flight — drop this stale refresh.
        if i64::from(g.get_album_id()) != album_id {
            return;
        }

        // Apply the user's *current* sort to the freshly-fetched rows.
        let field = g.get_sort_field().to_string();
        let dir = g.get_sort_dir().to_string();
        sort_track_list_rows(&mut tracks, &field, &dir);

        // Header is one row — always refresh it (artwork / counts may
        // have changed).
        g.set_album(to_slint_album_row(&detail));
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Albums);
        crate::ui::hero_chips::publish_album(&ui, &detail, genre.as_deref(), on_screen);
        // No fade on the refresh path — this is the same album, the
        // user did not navigate. Either it's a cache hit (no change) or
        // the cover/blur is being replaced in place.
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, on_screen);

        // Prune `selected-ids` to ids that still exist, then let the shared filter pass
        // re-derive the displayed cache and the model from the canonical set. It diffs, so a
        // refresh that leaves the id order alone patches only the rows whose content moved —
        // a tag edit, a favourite toggled elsewhere — and keeps the shift-range anchor.
        prune_selection_to(&g, &tracks);
        *albums_ui.detail.all_tracks.lock() = tracks;
        apply_filtered_detail(&ui, &albums_ui);
    });
    Ok(())
}

/// Re-sort the cached detail tracks and reorder the existing model rows to
/// match. No DB hit, **and no row rebuild** — a header click changes row order,
/// not row content, so the `UiTrackListRow` structs are moved rather than rebuilt
/// and nothing is re-decoded or re-allocated. Selection survives, track ids being
/// stable across a re-sort. Runs on the UI thread.
pub fn resort_detail(ui: &AppWindow, albums_ui: &AlbumsUi) {
    let g = ui.global::<AlbumDetail>();
    let field = g.get_sort_field().to_string();
    let dir = g.get_sort_dir().to_string();

    // Caches first — `play-row` and range-select read the displayed `tracks`,
    // and `all_tracks` sorts in lockstep so widening the filter later still
    // yields sorted rows.
    let order: Vec<i32> = {
        sort_track_list_rows(&mut albums_ui.detail.all_tracks.lock(), &field, &dir);
        let mut tracks = albums_ui.detail.tracks.lock();
        sort_track_list_rows(&mut tracks, &field, &dir);
        tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect()
    };

    crate::ui::model_diff::permute_rows_by_id(&g.get_tracks(), &order, |r| r.id);
    // A no-op in the steady state, the reordered structs keeping their flags;
    // kept as a cheap re-sync.
    apply_selection_to_rows(&g, albums_ui);
}

/// Clear the detail view's cached state when the user navigates back to
/// the grid. The Slint side has already flipped `album-id` to `-1`.
pub fn clear_detail(albums_ui: &AlbumsUi) {
    *albums_ui.detail.album_id.lock() = -1;
    albums_ui.detail.tracks.lock().clear();
    albums_ui.detail.all_tracks.lock().clear();
    albums_ui.detail.applied_selection.lock().clear();
    albums_ui.detail.filter.lock().clear();
}

/// Update the cached filter needle, so `refresh_detail` can re-apply it to fresh
/// data without round-tripping the UI thread for the property read. Stored
/// folded, so the per-keystroke walk doesn't re-fold per row.
pub fn set_filter(albums_ui: &AlbumsUi, needle: &str) {
    *albums_ui.detail.filter.lock() = crate::ui::row_match::fold_needle(needle);
}

/// Re-walk the cached tracks through the current filter and push the model.
/// Runs on the UI thread; [`crate::ui::detail_filter`] is the shared body.
pub fn apply_filtered_detail(ui: &AppWindow, albums_ui: &AlbumsUi) {
    let g = ui.global::<AlbumDetail>();
    crate::ui::detail_filter::apply_filtered_detail(
        &g,
        &FilterRefs {
            all_tracks: &albums_ui.detail.all_tracks,
            tracks: &albums_ui.detail.tracks,
            applied: &albums_ui.detail.applied_selection,
            filter: &albums_ui.detail.filter,
        },
    );
}

/// Flip `is_favorite` on one detail row, leaving scroll position and neighbours
/// alone. Mirrors `tracks::apply_row_favorite`.
pub fn apply_detail_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<AlbumDetail>().get_tracks(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// Set `rating` on a single detail row in the Slint `VecModel`. Mirrors
/// [`apply_detail_row_favorite`].
pub fn apply_detail_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<AlbumDetail>().get_tracks(), id, |r| {
            r.rating = rating;
        });
    });
}

/// Reopen the album that was visible at the last shutdown. Runs once at startup
/// *after* [`super::install`], so the `AlbumDetail` callbacks are live by the
/// time `open_album`'s closure lands. A deleted album just logs and leaves
/// `album-id` at `-1`, so the grid renders.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let Some(id) = library::settings::get_view_state(state).ok().and_then(|s| {
        s.last_detail_ids.get(crate::ui::track_list_view::view_id::ALBUM_DETAIL).copied()
    }) else {
        return;
    };
    // Synchronously, so it is up before `app.show()` — see `AlbumDetail.restoring`.
    ui.global::<AlbumDetail>().set_restoring(true);
    let s = state.clone();
    let au = albums_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // `Below` is the first-launch fade-up rather than a drill-in slide —
        // nobody navigated, this is a restore.
        if let Err(e) = open_album(&s, &au, weak.clone(), id, NavEnterFrom::Below).await {
            log::warn!("albums::seed_detail_from_settings open_album({id}): {e}");
        }
        // Lowered however it went, and behind `open_album`'s own hop so the id is already in: an
        // album gone since the last session owes the grid back rather than an empty body.
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<AlbumDetail>().set_restoring(false);
        });
    });
}

/// Clear the `selected-ids` model + anchor without walking the row model —
/// freshly-built rows already carry `selected: false`, so the `applied`
/// shadow is reset to empty to match.
pub(super) fn reset_detail_selection(g: &AlbumDetail, albums_ui: &AlbumsUi) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(-1);
    albums_ui.detail.applied_selection.lock().clear();
}
