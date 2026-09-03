//! The Playlists grid's filter + sort pass — the last of the four entity
//! grids to get one. Same three cases its siblings pin, since all four run
//! the identical `row_match::Needle::contains` walk over a single name.
//!
//! Plus the four things drag-to-reorder needs to stay alive, none of which any
//! other pin can see go missing.

use super::detail::is_manual_order;
use super::*;
use crate::test_support::{normalize_ws, strip_line_comments};

const DRAGGABLE_LIST: &str =
    include_str!("../../../../../melodia-ui/ui/components/track-list/draggable-track-list.slint");
const QUEUE_SHEET: &str = include_str!("../../../../../melodia-ui/ui/views/queue-sheet.slint");
const DETAIL_VIEW: &str =
    include_str!("../../../../../melodia-ui/ui/views/my-library/playlist-detail.slint");
const DETAIL_CALLBACKS: &str = include_str!("../callbacks/detail.rs");
const DETAIL: &str = include_str!("../detail.rs");

/// Minimal `PlaylistStats` builder — only the fields the grid filter / sort
/// read matter. Regular playlist, so `smart_criteria` never parses.
fn playlist(id: i64, name: &str, track_count: i32, updated_at: &str) -> PlaylistStats {
    PlaylistStats {
        id,
        name: name.to_owned(),
        description: None,
        thumbnail_path: None,
        is_smart: false,
        smart_criteria: None,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: updated_at.to_owned(),
        custom_thumbnail: false,
        track_count,
        total_duration_ms: 0,
    }
}

fn names(data: &GridData, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|&i| data.playlists[i].name.clone()).collect()
}

#[test]
fn compute_indices_with_empty_filter_keeps_all_playlists() {
    let data = GridData::new(vec![
        playlist(1, "Alpha", 1, "2026-01-02T00:00:00Z"),
        playlist(2, "Bravo", 1, "2026-01-01T00:00:00Z"),
    ]);
    let idx = compute_indices(&data, "name", "asc", "");
    assert_eq!(names(&data, &idx), ["Alpha", "Bravo"]);
}

#[test]
fn compute_indices_filter_matches_name_case_insensitively() {
    let data = GridData::new(vec![
        playlist(1, "Late Night Drive", 1, "2026-01-03T00:00:00Z"),
        playlist(2, "Night Shift", 1, "2026-01-02T00:00:00Z"),
        playlist(3, "Morning Coffee", 1, "2026-01-01T00:00:00Z"),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "NIGHT");
    assert_eq!(names(&data, &by_name), ["Late Night Drive", "Night Shift"]);
}

#[test]
fn compute_indices_filter_ignores_accents_the_way_the_search_view_does() {
    // A playlist name is whatever the user typed, so it carries whatever
    // their keyboard makes easy — and the query later may not.
    let data = GridData::new(vec![
        playlist(1, "Café Sessions", 1, "2026-01-02T00:00:00Z"),
        playlist(2, "Morning Coffee", 1, "2026-01-01T00:00:00Z"),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "cafe");
    assert_eq!(names(&data, &by_name), ["Café Sessions"]);
}

#[test]
fn compute_indices_default_sort_is_most_recently_updated_first() {
    // `playlist_stats` already returns `updated_at DESC`, so the default arm
    // re-derives that order rather than leaving whatever the filter produced.
    let data = GridData::new(vec![
        playlist(1, "Oldest", 1, "2026-01-01T00:00:00Z"),
        playlist(2, "Newest", 1, "2026-03-01T00:00:00Z"),
        playlist(3, "Middle", 1, "2026-02-01T00:00:00Z"),
    ]);
    let desc = compute_indices(&data, "updated", "desc", "");
    assert_eq!(names(&data, &desc), ["Newest", "Middle", "Oldest"]);
    let asc = compute_indices(&data, "updated", "asc", "");
    assert_eq!(names(&data, &asc), ["Oldest", "Middle", "Newest"]);
}

/// A row drag and the list's own drag-pan are one gesture, and whichever element
/// owns it the other gets nothing: a pan intercepts the row's grab mid-drag, so
/// the drop never commits. Both scrollers around the rows opt out, and so does
/// the queue sheet's list. Nothing else catches a regression here — it compiles,
/// reads clean, and only misbehaves on a list long enough to scroll.
///
/// The value was `!reorder-enabled`, which reads as leaving a `true` default alone
/// and is not one: every style but Material publishes this off, so that spelling
/// *enabled* the pan on every sort that retires the drag.
#[test]
fn every_draggable_list_opts_out_of_drag_panning() {
    // The binding rather than the token: the property reads as an *enable*, so
    // `mouse-drag-pan-enabled: true` is the likeliest wrong edit and a bare
    // occurrence count can't fail on it.
    let list = normalize_ws(&strip_line_comments(DRAGGABLE_LIST));
    assert_eq!(
        list.matches("mouse-drag-pan-enabled: false").count(),
        2,
        "both `outer-scroll` and `inner-list` must opt out — a diagonal drag steals the \
         grab on either axis once the columns overflow"
    );
    assert!(
        normalize_ws(&strip_line_comments(QUEUE_SHEET)).contains("mouse-drag-pan-enabled: false"),
        "every row in the queue sheet is draggable, so its ListView never gets the gesture"
    );
}

/// All four terms, since each fails differently and only the first is obvious.
/// `sort_playlist_tracks` reverses on `"desc"`, and the drag writes display
/// indices straight into `position_order`, so a reversed position sort lands
/// every drop at the mirrored slot; a filter makes those indices name a
/// different track entirely; and a smart playlist has no curated order to
/// reorder into. Dropping one of the last three costs no wrong write —
/// `apply_optimistic_reorder` refuses the filtered case and the query the empty
/// item set — but the drag then starts, paints a ghost and a drop line, and does
/// nothing on release, which is worse than a list that never armed.
#[test]
fn the_reorder_gate_reads_every_term_the_drag_depends_on() {
    let src = normalize_ws(&strip_line_comments(DETAIL_VIEW));
    assert!(
        src.contains(
            "reorder-enabled: PlaylistDetail.sort-field == \"position\" \
             && PlaylistDetail.sort-dir != \"desc\" \
             && PlaylistDetail.filter == \"\" \
             && !PlaylistDetail.playlist.is_smart"
        ),
        "playlist-detail.slint must gate reorder on an *ascending* position sort, \
         an unfiltered list and a playlist with a curated order"
    );
}

/// `"position"` is what drag-to-reorder is gated on and no header cell asks for
/// it, so the sort cycle is the only way back. Drop the natural field and the
/// first click on any column retires reordering for the whole install —
/// persisted, so it survives a restart.
#[test]
fn the_sort_cycle_still_offers_a_way_back_to_the_curated_order() {
    let src = normalize_ws(&strip_line_comments(DETAIL_CALLBACKS));
    assert!(
        src.contains("next_sort_with_natural"),
        "the plain `next_sort` has two states and cannot reach `\"position\"`"
    );
    assert!(
        src.contains("Some(playlists_ui_mod::POSITION_FIELD)"),
        "the cycle needs the curated order named as its third state"
    );
}

/// `is_manual_order` answers two of `reorder-enabled`'s terms; the filter is the
/// third the drag depends on, and the one that fails silently — filtered,
/// `tracks` is a subset of `position_order`, so a display index is still in
/// range and the DB write lands on a different track than the one dragged.
///
/// Scoped to the guard rather than searched for over the file, so a later site
/// reading the same field elsewhere in `detail.rs` can't satisfy the assertion
/// while the guard itself has stopped asking. Each needle carries its operator
/// for the same reason the drag-pan pin above matches a binding — a dropped `!`
/// or an `&&` in place of the `||` inverts the guard, and a bare token match
/// can't see either.
#[test]
fn the_optimistic_reorder_refuses_a_filtered_list() {
    let src = normalize_ws(&strip_line_comments(DETAIL));
    let body = src.split_once("pub fn apply_optimistic_reorder").map_or("", |(_, rest)| rest);
    let guard = body.split_once("let saved =").map_or("", |(head, _)| head);
    assert!(
        guard.contains("!is_manual_order(&field, &dir)"),
        "the order half must be asked before the display indices reach `position_order`"
    );
    assert!(
        guard.contains("|| !playlists_ui.detail.filter.lock().is_empty()"),
        "so must the filter half — a filtered index is in range, so the write lands"
    );
}

#[test]
fn only_an_ascending_position_sort_is_the_manual_order() {
    assert!(is_manual_order(POSITION_FIELD, "asc"));
    assert!(!is_manual_order(POSITION_FIELD, "desc"));
    assert!(!is_manual_order("title", "asc"));
    assert!(!is_manual_order("title", "desc"));
    // `SortDir::from_token`'s rule — only `"desc"` is descending.
    assert!(is_manual_order(POSITION_FIELD, "sideways"));
}
