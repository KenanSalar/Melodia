//! Column visibility and widths across the `views.json` round trip, driven through a stand-in for
//! a view's Slint global.

use std::cell::RefCell;

use super::*;

/// A track-list global without a window, hiding `#` and Genre on a first launch and locking Album.
#[derive(Default)]
struct FakeList(RefCell<TrackColumns>);

impl TrackListColumnState for FakeList {
    const VIEW_ID: &'static str = "fake";
    const HIDDEN_BY_DEFAULT: &'static [&'static str] = &["number", "genre"];
    const LOCKED: Option<&'static str> = Some("album");

    fn read_columns(&self) -> TrackColumns {
        self.0.borrow().clone()
    }

    fn write_columns(&self, columns: TrackColumns) {
        *self.0.borrow_mut() = columns;
    }
}

/// The toggleable columns' visibility in [`TOGGLEABLE`] order: `#`, artwork, Artist, Album,
/// Genre, Year.
fn shown(list: &FakeList) -> [bool; 6] {
    let c = list.read_columns();
    [c.show_number, c.show_artwork, c.show_artist, c.show_album, c.show_genre, c.show_year]
}

fn saved(visible: &[&str]) -> ViewStateData {
    let mut vs = ViewStateData::default();
    vs.view_columns
        .insert(FakeList::VIEW_ID.to_owned(), visible.iter().map(|&id| id.to_owned()).collect());
    vs
}

fn hydrated(vs: &ViewStateData) -> FakeList {
    let list = FakeList::default();
    hydrate(vs, &list);
    list
}

#[test]
fn a_view_with_nothing_saved_hides_its_first_launch_columns() {
    let list = hydrated(&ViewStateData::default());
    assert_eq!(shown(&list), [false, true, true, false, false, true]);
}

/// Unticking every column is a choice the next launch has to keep, not a missing entry that falls
/// back to the defaults.
#[test]
fn a_saved_empty_set_hides_every_toggleable_column() {
    let list = hydrated(&saved(&[]));
    assert_eq!(shown(&list), [false; 6]);
}

#[test]
fn a_locked_column_stays_hidden_even_when_saved_visible() {
    let list = hydrated(&saved(&["album", "artist"]));
    assert_eq!(shown(&list), [false, false, true, false, false, false]);
}

#[test]
fn an_id_the_column_popup_does_not_offer_is_ignored() {
    let list = hydrated(&saved(&["title", "bogus", "year"]));
    assert_eq!(shown(&list), [false, false, false, false, false, true]);
}

#[test]
fn a_snapshot_names_the_visible_columns_in_display_order_without_the_locked_one() {
    let list = FakeList(RefCell::new(TrackColumns {
        show_year: true,
        show_album: true,
        show_number: true,
        ..TrackColumns::default()
    }));
    assert_eq!(list.snapshot_visible(), ["number", "year"]);
}

#[test]
fn what_a_snapshot_saves_hydrates_back_into_the_same_columns() {
    let mut vs = saved(&["artist", "year"]);
    let widths = ColumnWidths { title: 410.0, year: 64.0, ..ColumnWidths::default() };
    vs.view_column_widths.insert(FakeList::VIEW_ID.to_owned(), widths);
    let first = hydrated(&vs);

    let mut resaved = ViewStateData::default();
    snapshot(&mut resaved, &first);

    assert_eq!(hydrated(&resaved).read_columns(), first.read_columns());
}

#[test]
fn a_snapshot_leaves_every_other_views_entries_alone() {
    let mut vs = ViewStateData::default();
    vs.view_columns.insert(view_id::TRACKS.to_owned(), vec!["genre".to_owned()]);

    snapshot(&mut vs, &hydrated(&ViewStateData::default()));

    assert_eq!(vs.view_columns.get(view_id::TRACKS), Some(&vec!["genre".to_owned()]));
}

type ShippedRow = (&'static str, &'static [&'static str], Option<&'static str>);

fn shipped<T: TrackListColumnState>() -> ShippedRow {
    (T::VIEW_ID, T::HIDDEN_BY_DEFAULT, T::LOCKED)
}

/// The first-launch columns every view ships with. The ids are spelled as bare identifiers in
/// `track_list_views!`, so a misspelled one compiles and quietly shows the column it meant to hide.
#[test]
fn every_view_hides_and_locks_the_columns_it_shipped_with() {
    let rows = [
        shipped::<Tracks<'static>>(),
        shipped::<Browse<'static>>(),
        shipped::<AlbumDetail<'static>>(),
        shipped::<ArtistDetail<'static>>(),
        shipped::<GenreDetail<'static>>(),
        shipped::<PlaylistDetail<'static>>(),
        shipped::<Favorites<'static>>(),
        shipped::<RecentlyPlayed<'static>>(),
        shipped::<Search<'static>>(),
    ];

    let expected: [ShippedRow; 9] = [
        ("tracks", &[], None),
        ("browse", &[], None),
        ("album_detail", &[], Some("album")),
        ("artist_detail", &["number"], Some("artist")),
        ("genre_detail", &["number"], Some("genre")),
        ("playlist_detail", &["genre"], None),
        ("favorites", &["number", "genre", "year"], None),
        ("recently_played", &["number", "genre", "year"], None),
        ("search", &["genre", "year"], None),
    ];
    assert_eq!(rows, expected);
}
