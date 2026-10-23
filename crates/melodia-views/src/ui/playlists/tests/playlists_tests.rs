//! What drag-to-reorder and the row menu's removal need to stay alive, none of
//! which any other pin can see go missing.

use super::detail::{POSITION_FIELD, is_manual_order};
use melodia_testkit::code_tokens;

const DRAGGABLE_LIST: &str =
    include_str!("../../../../../melodia-ui/ui/components/track-list/draggable-track-list.slint");
const QUEUE_PANEL: &str =
    include_str!("../../../../../melodia-ui/ui/views/queue-panel/queue-panel.slint");
const DETAIL_VIEW: &str =
    include_str!("../../../../../melodia-ui/ui/views/my-library/playlist-detail.slint");
const SHARED_DETAIL_WIRING: &str = include_str!("../../callbacks/track_detail.rs");
const DETAIL: &str = include_str!("../detail.rs");
const DETAIL_CALLBACKS: &str = include_str!("../callbacks/detail.rs");

/// A list's drag and its own drag-pan are one gesture, and whichever element owns
/// it the other gets nothing. The list around the rows opts out, and so does the
/// queue panel's. Nothing else catches a regression here — it compiles,
/// reads clean, and only misbehaves on a list long enough to scroll.
///
/// The value was `!reorder-enabled`, which reads as leaving a `true` default alone
/// and is not one: every style publishes this off, so that spelling
/// *enabled* the pan on every sort that retires the drag.
#[test]
fn every_draggable_list_opts_out_of_drag_panning() {
    // The binding rather than the token: the property reads as an *enable*, so
    // `mouse-drag-pan-enabled: true` is the likeliest wrong edit and a bare
    // occurrence count can't fail on it.
    let list = code_tokens(DRAGGABLE_LIST);
    assert_eq!(
        list.matches("mouse-drag-pan-enabled: false").count(),
        1,
        "`inner-list` must opt out, the only scroller the rows sit in"
    );
    assert!(
        code_tokens(QUEUE_PANEL).contains("mouse-drag-pan-enabled: false"),
        "the queue panel's list drags, so its ListView never gets the gesture"
    );
}

/// All four terms, since each fails differently and only the first is obvious.
/// A descending position sort shows the playlist reversed, and the drag moves
/// display indices straight in the keyset, so a reversed position sort lands
/// every drop at the mirrored slot; a filter makes those indices name a
/// different track entirely; and a smart playlist has no curated order to
/// reorder into. Dropping one of the last three costs no wrong write —
/// `apply_optimistic_move` refuses the filtered case and the query the empty
/// item set — but the drag then starts, paints a ghost and a drop line, and does
/// nothing on release, which is worse than a list that never armed.
#[test]
fn the_reorder_gate_reads_every_term_the_drag_depends_on() {
    let src = code_tokens(DETAIL_VIEW);
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
///
/// Two halves in two files: the shared wiring has to hand the view's natural
/// order to the cycle, and this view has to name one.
#[test]
fn the_sort_cycle_still_offers_a_way_back_to_the_curated_order() {
    assert!(
        code_tokens(SHARED_DETAIL_WIRING)
            .contains("next_sort_with_natural(&field, &dir, &clicked, V::NATURAL_SORT)"),
        "the plain `next_sort` has two states and cannot reach `\"position\"`"
    );
    assert!(
        code_tokens(DETAIL)
            .contains("const NATURAL_SORT: Option<&'static str> = Some(POSITION_FIELD);"),
        "the cycle needs the curated order named as its third state"
    );
}

/// `is_manual_order` answers two of `reorder-enabled`'s terms; the filter is the
/// third the drag depends on, and the one that fails silently — filtered, the
/// list is a subset of the playlist, so a display index is still in range and
/// the DB write lands on a different track than the one dragged.
///
/// Scoped to the guard rather than searched for over the file, so a later site
/// reading the same field elsewhere in `detail.rs` can't satisfy the assertion
/// while the guard itself has stopped asking. Each needle carries its operator
/// for the same reason the drag-pan pin above matches a binding — a dropped `!`
/// or an `&&` in place of the `||` inverts the guard, and a bare token match
/// can't see either.
#[test]
fn the_optimistic_reorder_refuses_a_filtered_list() {
    let src = code_tokens(DETAIL);
    let guard = reorder_guard(&src);
    assert!(
        guard.contains("!is_manual_order(&field, &dir)"),
        "the order half must be asked before the display indices reach the keyset"
    );
    assert!(
        guard.contains("|| playlists_ui.detail.is_filtered()"),
        "so must the filter half — a filtered index is in range, so the write lands"
    );
}

/// The two terms above read what the page asked for, which a list shows a load later, and an
/// overtaken open leaves another playlist's rows up under this one's id. Only the list on show
/// can say its indices are positions.
#[test]
fn the_optimistic_reorder_refuses_a_list_not_shown_in_the_playlists_own_order() {
    let src = code_tokens(DETAIL);
    assert!(
        reorder_guard(&src).contains("|| !playlists_ui.detail.shows_own_order()"),
        "the guard must ask the list on show, not only the sort and filter the page asked for"
    );
}

/// `apply_optimistic_move` up to where the display indices reach the keyset.
fn reorder_guard(src: &str) -> &str {
    let body = src.split_once("pub fn apply_optimistic_move").map_or("", |(_, rest)| rest);
    body.split_once("let tracks =").map_or("", |(head, _)| head)
}

/// The reload a removal brings prunes nothing under a filter, so the removal drops its own rows
/// and their picks, past the write's early return so a failed write leaves both alone.
#[test]
fn a_removal_drops_its_rows_and_their_picks_once_the_write_lands() {
    let src = code_tokens(DETAIL_CALLBACKS);
    let after_write = src
        .split_once("fn wire_remove_track(")
        .and_then(|(_, body)| body.split_once("remove_tracks_from_playlist_batch("))
        .map_or("", |(_, rest)| rest);
    assert!(
        after_write.contains("remove_rows(&*pu, id_vec);"),
        "left to the reload, a filtered removal keeps its picks out of sight"
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
