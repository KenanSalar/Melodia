//! Every write and clear the Search view's models take. Rows are built on the worker, and only
//! what needs the UI thread, the translated plurals, waits for the event-loop closure.

use std::sync::Arc;

use slint::{ComponentHandle, SharedString, Weak};

use super::state::COMPACT_TRACK_LIMIT;
use super::top_result::{TopKind, TopResult, TopSubtitle, compute_top_result};
use super::{SearchUi, to_slint_album_strip_row, to_slint_artist_strip_row};
use crate::ui::genres::genre_accent;
use crate::ui::list_selection;
use crate::ui::model_diff;
use crate::ui::model_patch;
use crate::ui::track_sort::sort_track_rows_by;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32, opt_shared};
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::search::SearchResults;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::entities::track_list::SortDir;
use melodia_ui::{
    AppWindow, EntityStripRow as UiEntityStripRow, Search, TrackListRow as UiTrackListRow,
};

/// Writes `results` into every surface, cutting the Songs slice against the live
/// `show-all-tracks`.
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
    // Artist rows wait for the closure, their subtitle being a plural only the UI thread can
    // translate.
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

/// Sorts the tracks into display order and builds every row on the worker: the compact cut reads
/// UI-thread state, and a LIMIT-bounded set is cheap to build whole.
fn sort_track_rows(
    tracks: &[RsTrackListRow],
    sort: &melodia_app::services::settings::ViewSort,
) -> (Vec<UiTrackListRow>, i32) {
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
    (crate::ui::tracks::to_slint_track_list_rows(&sorted), total)
}

/// Cuts the Songs rows against the live `show-all-tracks` and writes them, the uncut total
/// beside them.
fn write_tracks(g: &Search, rows: Vec<UiTrackListRow>, total: i32) {
    // Against the whole result set: the compact view hides the rest rather than dropping it.
    list_selection::keep_selection_within(g, || rows.iter().map(|row| row.id).collect());
    let take =
        if g.get_show_all_tracks() { rows.len() } else { rows.len().min(COMPACT_TRACK_LIMIT) };
    let mut shown: Vec<UiTrackListRow> = rows.into_iter().take(take).collect();
    list_selection::restamp_row_selection(g, &mut shown);
    model_diff::replace_rows(&g.get_tracks(), shown, "Search.tracks");
    g.set_tracks_total(total);
}

fn write_strips(g: &Search, album_rows: Vec<UiEntityStripRow>, artists: &[ArtistStats]) {
    model_diff::replace_rows(&g.get_album_rows(), album_rows, "Search.album-rows");
    let artist_rows: Vec<UiEntityStripRow> = artists
        .iter()
        .map(|a| to_slint_artist_strip_row(a, &g.invoke_album_count_label(a.album_count)))
        .collect();
    model_diff::replace_rows(&g.get_artist_rows(), artist_rows, "Search.artist-rows");
}

/// Paints the Top Result card, or clears it when nothing ranked.
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
    g.set_top_artwork_path(opt_shared(t.artwork_path.as_deref()));
    // From the name, as the genre's grid card derives it, so the two tint alike. The view reads
    // these only under `top-kind == "genre"`.
    if t.kind == TopKind::Genre {
        let accent = genre_accent(&t.title);
        g.set_top_tile_color_1(accent.tile_color_1);
        g.set_top_tile_color_2(accent.tile_color_2);
    }
}

/// Emptying `top-artwork-path` releases the image the card holds through its cover callback, so
/// no image write is owed.
fn clear_top_result(g: &Search) {
    g.set_top_kind(SharedString::default());
    g.set_top_id(-1);
    g.set_top_title(SharedString::default());
    g.set_top_subtitle(SharedString::default());
    g.set_top_artwork_path(SharedString::default());
}

/// Empties every results surface, as a view with no query shows.
fn clear_result_models(g: &Search) {
    model_diff::clear_vec_model(&g.get_tracks(), "Search.tracks");
    model_diff::clear_vec_model(&g.get_album_rows(), "Search.album-rows");
    model_diff::clear_vec_model(&g.get_artist_rows(), "Search.artist-rows");
    g.set_tracks_total(0);
    clear_top_result(g);
    g.set_loading(false);
}

/// The empty-query path, so the recent searches show as soon as the box empties.
pub fn clear_results_on_ui(weak: &Weak<AppWindow>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        clear_result_models(&ui.global::<Search>());
    });
}

/// Patches the one row on screen, so the scroll holds.
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Search>().get_tracks(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// [`apply_row_favorite`]'s star-rating twin.
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Search>().get_tracks(), id, |r| {
            r.rating = rating;
        });
    });
}

pub fn set_loading_on_ui(weak: &Weak<AppWindow>, on: bool) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.global::<Search>().set_loading(on);
    });
}

/// The leave's UI half, run before the blocking release drops the tiers so no model still holds
/// their images. Keeps `query` and `recent-rows`, so a flip away and back keeps the user's typing.
pub fn teardown_models_on_leave(ui: &AppWindow) {
    let g = ui.global::<Search>();
    clear_result_models(&g);
    g.set_show_all_tracks(false);
    model_diff::clear_vec_model(&g.get_selected_ids(), "Search.selected-ids");
    g.set_selection_anchor(list_selection::NO_ANCHOR);
}
