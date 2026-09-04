//! Every write the Search view's models and scalars take, and every clear.
//!
//! The split from [`super::fetch`] is worker-versus-UI-thread as much as it
//! is by job: the row halves that are `Send` are prepared off-thread and only
//! the `!Send` cover lookups and the translated plurals happen inside the
//! event-loop closure.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::state::COMPACT_TRACK_LIMIT;
use super::top_result::{TopKind, TopResult, TopSubtitle, compute_top_result};
use super::{SearchUi, restamp_rows, to_slint_album_strip_row, to_slint_artist_strip_row};
use crate::ui::genres::genre_accent;
use crate::ui::track_sort::sort_track_rows_by;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use melodia_app::services::settings::SortDir;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::search::SearchResults;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_ui::{
    AppWindow, EntityStripRow as UiEntityStripRow, Search, TrackListRow as UiTrackListRow,
};

/// Push the freshly-fetched `results` into all four Slint models,
/// compute + apply the Top Result, set `tracks-total`, and honour the
/// `show-all-tracks` toggle (the apply path always re-derives the
/// visible Songs slice — that way callers don't have to remember to
/// reset `show-all-tracks` themselves).
pub fn apply_results_to_slint(
    search_ui: &Arc<SearchUi>,
    weak: &Weak<AppWindow>,
    results: &SearchResults,
    query: &str,
) {
    let sort = search_ui.state().sort.lock().clone();
    let (rows, total) = sort_track_rows(&results.tracks, &sort);
    let top = compute_top_result(results, query);

    let album_rows: Vec<UiEntityStripRow> =
        results.albums.iter().map(to_slint_album_strip_row).collect();
    // Artist rows can't be finished here — their subtitle is a translated
    // plural that only `Search.album-count-label` resolves, and that is a
    // UI-thread callback. Carry the entities across and build the rows
    // inside the closure below.
    let artists: Vec<ArtistStats> = results.artists.clone();

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Search>();
        write_tracks(&g, rows, total);
        write_strips(&g, album_rows, &artists);
        write_top_result(&g, top);
    });
}

/// Sort a result set's tracks into display order and build their rows. Runs on the worker, so
/// the event-loop closure only pays for the model write.
///
/// Result sets are LIMIT-bounded, so building the *full* set — rather than just the compact
/// slice, which can't be sized off-thread because `show-all-tracks` is UI-thread state — is
/// cheap.
fn sort_track_rows(
    tracks: &[RsTrackListRow],
    sort: &melodia_app::services::settings::ViewSort,
) -> (Vec<UiTrackListRow>, i32) {
    // The FTS5 `ORDER BY rank` lands these rank-ordered, but the user may
    // have picked a different sort after the fact.
    let mut sorted: Vec<RsTrackListRow> = tracks.to_vec();
    if sort.field == "rank" {
        if matches!(sort.dir, SortDir::Desc) {
            sorted.reverse();
        }
    } else {
        sort_track_rows_by(
            &mut sorted,
            sort.field.as_str(),
            match sort.dir {
                SortDir::Asc => "asc",
                SortDir::Desc => "desc",
            },
            |t| t,
            |t| t.title.to_lowercase(),
        );
    }

    let total = len_as_i32(sorted.len());
    let rows = sorted.iter().map(crate::ui::tracks::to_slint_track_list_row).collect();
    (rows, total)
}

/// Truncate the Songs rows against the live `show-all-tracks` flag and write
/// them, with the untruncated total beside them.
fn write_tracks(g: &Search, rows: Vec<UiTrackListRow>, total: i32) {
    let take = if g.get_show_all_tracks() {
        rows.len()
    } else {
        rows.len().min(COMPACT_TRACK_LIMIT)
    };
    let mut shown: Vec<UiTrackListRow> = rows.into_iter().take(take).collect();
    restamp_rows(g, &mut shown);
    write_track_model(g, shown);
    g.set_tracks_total(total);
}

/// Write both entity strips. The artist rows are built here rather than on
/// the worker because their subtitle is a translated plural.
fn write_strips(g: &Search, album_rows: Vec<UiEntityStripRow>, artists: &[ArtistStats]) {
    write_strip(&g.get_album_rows(), album_rows, "album");
    let artist_rows: Vec<UiEntityStripRow> = artists
        .iter()
        .map(|a| to_slint_artist_strip_row(a, &g.invoke_album_count_label(a.album_count)))
        .collect();
    write_strip(&g.get_artist_rows(), artist_rows, "artist");
}

/// Paint the Top Result card, or clear it when nothing ranked.
fn write_top_result(g: &Search, top: Option<TopResult>) {
    let Some(t) = top else {
        clear_top_result(g);
        return;
    };
    g.set_top_kind(SharedString::from(match t.kind {
        TopKind::Album => "album",
        TopKind::Artist => "artist",
        TopKind::Genre => "genre",
    }));
    g.set_top_id(clamp_i64_to_i32(t.id));
    g.set_top_title(SharedString::from(t.title.as_str()));
    g.set_top_subtitle(match &t.subtitle {
        TopSubtitle::Text(s) => SharedString::from(s.as_str()),
        TopSubtitle::AlbumCount(n) => g.invoke_album_count_label(*n),
        TopSubtitle::TrackCount(n) => g.invoke_track_count_label(*n),
    });
    g.set_top_artwork_path(SharedString::from(t.artwork_path.as_deref().unwrap_or("")));
    // Derived from the title rather than carried on `TopResult`:
    // `genre_accent` is a pure function of the name, so deriving here is what
    // guarantees this card and the genre's grid card tint identically. The
    // view reads these only under `top-kind == "genre"`, so the other kinds
    // need no write.
    if t.kind == TopKind::Genre {
        let accent = genre_accent(&t.title);
        g.set_top_tile_color_1(accent.tile_color_1);
        g.set_top_tile_color_2(accent.tile_color_2);
    }
}

/// Blank the Top Result card. Emptying `top-artwork-path` is what releases the
/// `Image` slot the card holds through its cover callback — the binding won't
/// be re-evaluated for an empty path, so no explicit image write is owed.
fn clear_top_result(g: &Search) {
    g.set_top_kind(SharedString::from(""));
    g.set_top_id(-1);
    g.set_top_title(SharedString::from(""));
    g.set_top_subtitle(SharedString::from(""));
    g.set_top_artwork_path(SharedString::from(""));
}

/// Empty every results surface: the Songs model, both strips, the total, the
/// Top Result and the loading flag. What a view with no query shows.
fn clear_result_models(g: &Search) {
    write_track_model(g, Vec::new());
    write_strip(&g.get_album_rows(), Vec::new(), "album");
    write_strip(&g.get_artist_rows(), Vec::new(), "artist");
    g.set_tracks_total(0);
    clear_top_result(g);
    g.set_loading(false);
}

/// Empty-query fast path. Called synchronously from
/// [`super::fetch::kick_search`] so the recent-searches branch can render the
/// same tick the user clears the `SearchBar`.
pub fn clear_results_on_ui(weak: &Weak<AppWindow>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        clear_result_models(&ui.global::<Search>());
    });
}

/// Toggle the `loading` flag on the Slint side. Used by `kick_search`
/// to surface the "Searching…" line through the FTS round-trip.
pub fn set_loading_on_ui(weak: &Weak<AppWindow>, on: bool) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.global::<Search>().set_loading(on);
    });
}

/// UI-thread teardown — mirror of `release_section_state`'s Rust-side
/// cleanup, run synchronously *before* the `spawn_blocking` that drops
/// the LRUs. Clears every model + scalar + image slot the global owns
/// so the LRU's `SharedPixelBuffer` Arcs release on the same tick.
/// Keeps `Search.query` + `Search.recent-rows` so a brief flip away
/// and back doesn't lose the user's typing.
pub fn teardown_models_on_leave(ui: &AppWindow) {
    let g = ui.global::<Search>();
    clear_result_models(&g);
    g.set_show_all_tracks(false);
    // Drop the selection set too so the next session enter starts
    // clean. The user just navigated away — they can't be relying on
    // a sticky selection.
    clear_selected_ids_model(&g);
    g.set_selection_anchor(-1);
}

fn clear_selected_ids_model(g: &Search) {
    let model = g.get_selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(Vec::new());
    }
}

fn write_track_model(g: &Search, rows: Vec<UiTrackListRow>) {
    let model = g.get_tracks();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        log::warn!("Search.tracks: VecModel<TrackListRow> downcast failed");
        return;
    };
    vm.set_vec(rows);
}

fn write_strip(model: &slint::ModelRc<UiEntityStripRow>, rows: Vec<UiEntityStripRow>, label: &str) {
    let Some(vm) = model.as_any().downcast_ref::<VecModel<UiEntityStripRow>>() else {
        log::warn!("Search.{label}-rows: VecModel<EntityStripRow> downcast failed");
        return;
    };
    vm.set_vec(rows);
}
