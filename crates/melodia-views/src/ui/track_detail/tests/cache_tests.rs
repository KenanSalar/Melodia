//! Whether the list on show is a playlist in its own order, the half of the drag guard the page's
//! sort and filter can't answer: those move at a pick, and the list a load later.

use slint::Weak;

use super::DetailCache;
use crate::ui::paged_rows::ScrollPoint;
use melodia_core::entities::track_list::{SortDir, TrackOrder, TrackSortField};
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::PlaylistDetail;

/// The playlist the page has open.
const OPEN: i64 = 7;
/// The playlist the page was showing before it.
const LEFT: i64 = 5;

const OWN_ORDER: TrackOrder = TrackOrder { field: TrackSortField::Position, dir: SortDir::Asc };
const BY_TITLE: TrackOrder = TrackOrder { field: TrackSortField::Title, dir: SortDir::Asc };

fn cache() -> DetailCache<PlaylistDetail<'static>> {
    DetailCache::new(Weak::default(), ScrollPoint::default())
}

/// A cache seated on `id` in its own order, as an open leaves it before its keyset shows.
fn seated(id: i64) -> DetailCache<PlaylistDetail<'static>> {
    let cache = cache();
    cache.seat(id, OWN_ORDER);
    cache
}

#[test]
fn a_playlist_shown_by_position_unfiltered_is_in_its_own_order() {
    let cache = seated(OPEN);

    cache.record_shown(OPEN, OWN_ORDER, &Needle::default());

    assert!(cache.shows_own_order());
}

/// A descending sort would land every drop on the mirrored row, and a filtered list's indices name
/// other tracks than the positions they would be written as.
#[test]
fn a_list_shown_in_any_other_order_or_filtered_is_not() {
    let cases = [
        (TrackOrder { dir: SortDir::Desc, ..OWN_ORDER }, Needle::default(), "position, descending"),
        (BY_TITLE, Needle::default(), "title"),
        (OWN_ORDER, fold_needle("ha"), "position, filtered"),
    ];
    for (order, needle, case) in cases {
        let cache = seated(OPEN);

        cache.record_shown(OPEN, order, &needle);

        assert!(!cache.shows_own_order(), "{case}");
    }
}

/// The page asks for its own order a load before the list shows it, and a drag released in between
/// wrote the old list's indices as positions.
#[test]
fn asking_for_the_own_order_is_not_showing_it() {
    let cache = cache();
    cache.seat(OPEN, BY_TITLE);

    cache.set_order(OWN_ORDER);

    assert!(!cache.shows_own_order());
}

/// An overtaken open seats its id over the rows of the playlist it left, which a drag would reorder
/// by this one's positions.
#[test]
fn the_playlist_left_on_screen_is_not_the_open_ones_own_order() {
    let cache = seated(LEFT);
    cache.record_shown(LEFT, OWN_ORDER, &Needle::default());

    cache.seat(OPEN, OWN_ORDER);

    assert!(!cache.shows_own_order());
}

/// A close and a section leave empty the list.
#[test]
fn a_released_list_shows_no_order() {
    let cache = seated(OPEN);
    cache.record_shown(OPEN, OWN_ORDER, &Needle::default());

    cache.forget_shown();

    assert!(!cache.shows_own_order());
}
