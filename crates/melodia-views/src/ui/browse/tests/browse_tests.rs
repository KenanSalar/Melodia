//! Tests for the two things in `browse/mod.rs` that hold no UI handle.
//!
//! `BrowseUi` is a struct of mutexes and atomics, so its breadcrumb stack is reachable without a
//! window: what needs one is the fetch that fills it and the models it writes into. The stack and
//! the row conversion are the halves that decide, respectively, where Back lands and whether a
//! row can be interacted with at all.

use melodia_core::entities::track::TrackListRow;

use super::*;

fn browse_ui() -> BrowseUi {
    BrowseUi::new(Arc::new(CoverThumbs::new()))
}

/// A row as the library knows it. Only the fields the conversion carries or blanks are set to
/// anything distinguishable.
fn library_row(id: i64) -> TrackListRow {
    TrackListRow {
        id,
        file_path: "/m/ghost.flac".to_owned(),
        file_name: "ghost.flac".to_owned(),
        title: "Ghost Town".to_owned(),
        artist: Some("The Specials".to_owned()),
        album_artist: Some("Various Artists".to_owned()),
        album: Some("More Specials".to_owned()),
        genre: Some("Ska".to_owned()),
        track_number: Some(3),
        disc_number: None,
        year: Some(1981),
        artwork_path: Some("artwork/ab.jpg".to_owned()),
        duration_ms: 213_000,
        is_favorite: true,
        rating: 4,
        album_id: Some(7),
        artist_id: Some(8),
        genre_id: Some(9),
        date_added: "2026-01-01T00:00:00Z".to_owned(),
        sort_key: None,
    }
}

// --- the breadcrumb stack ------------------------------------------------

/// Drilling in and coming back out is the whole of what the stack is for, and `pop_history`
/// owns both halves: handing back where to go and landing there. A pop that returned the path
/// without moving `current_path` would leave Back reporting a directory the page never opens.
#[test]
fn a_pop_returns_to_the_directory_the_push_left() {
    let ui = browse_ui();
    ui.set_path("/music".to_owned());

    ui.push_history("/music".to_owned(), "/music/live".to_owned());
    assert_eq!(ui.current_path(), "/music/live");

    assert_eq!(ui.pop_history().as_deref(), Some("/music"));
    assert_eq!(ui.current_path(), "/music");
}

/// The root, where Back has nowhere to go. Answering `None` is what the caller reads to leave the
/// button inert; moving `current_path` anyway would strand the page above the library folders.
#[test]
fn a_pop_with_nothing_pushed_moves_nowhere() {
    let ui = browse_ui();
    ui.set_path("/music".to_owned());

    assert_eq!(ui.pop_history(), None);
    assert_eq!(ui.current_path(), "/music");
}

/// A breadcrumb click lands on an ancestor and drops everything below it, which is what makes the
/// next Back go to that ancestor's parent rather than back down the branch just left.
#[test]
fn a_breadcrumb_click_drops_everything_below_the_one_clicked() {
    let ui = browse_ui();
    ui.push_history("/music".to_owned(), "/music/live".to_owned());
    ui.push_history("/music/live".to_owned(), "/music/live/1998".to_owned());

    ui.truncate_history_to("/music");
    assert_eq!(ui.current_path(), "/music");

    assert_eq!(ui.pop_history(), None, "the ancestor clicked is now the bottom of the stack");
}

/// The case the method's own comment calls out: a breadcrumb that stopped being an ancestor
/// across a refresh. Clearing is the safe answer, because a stack still holding the old branch
/// would send Back into directories that are no longer above where the user is standing.
#[test]
fn a_breadcrumb_that_is_no_longer_an_ancestor_clears_the_stack() {
    let ui = browse_ui();
    ui.push_history("/music".to_owned(), "/music/live".to_owned());
    ui.push_history("/music/live".to_owned(), "/music/live/1998".to_owned());

    ui.truncate_history_to("/elsewhere");
    assert_eq!(ui.current_path(), "/elsewhere");

    assert_eq!(ui.pop_history(), None, "a stale branch must not stay reachable through Back");
}

/// A refresh re-lands on the directory already open and must not push it, or every
/// `library_changed` bump while Browse is visible would grow the stack by one and Back would
/// walk through the same directory repeatedly.
#[test]
fn setting_the_path_directly_leaves_the_stack_alone() {
    let ui = browse_ui();
    ui.push_history("/music".to_owned(), "/music/live".to_owned());

    ui.set_path("/music/live".to_owned());

    assert_eq!(ui.pop_history().as_deref(), Some("/music"));
}

// --- the row a disk-only file becomes ------------------------------------

/// The shared `TrackList` draws a row it can be interacted with only when `enabled`, and the
/// Tracks converter this branch delegates to does not set it. Left at its default, every row on
/// the Browse page is dimmed and swallows every click, which is the page not working at all.
#[test]
fn a_file_the_library_knows_is_interactive_and_keeps_its_id() {
    let file = BrowseFile {
        row: library_row(42),
        in_library: true,
    };

    let row = to_slint_browse_track_row(&file);

    assert!(row.enabled, "an in-library row the list will not let you click is a dead page");
    assert_eq!(row.id, 42);
    assert_eq!(row.title, "Ghost Town");
    assert_eq!(row.artist, "The Specials");
}

/// A file on disk with no database row behind it. `id == 0` and `enabled == false` are what the
/// row item reads to draw it dimmed and swallow interaction, and they have to travel together:
/// an enabled row carrying id 0 offers the user a play, a rating and a favourite that address no
/// track, and the write lands on whatever row id 0 resolves to or on nothing at all.
#[test]
fn a_file_the_library_does_not_know_is_sparse_and_inert() {
    let file = BrowseFile {
        row: library_row(42),
        in_library: false,
    };

    let row = to_slint_browse_track_row(&file);

    assert!(!row.enabled, "a row with no track behind it must not accept interaction");
    assert_eq!(row.id, 0, "an id from the disk-only branch would address someone else's track");
    assert_eq!(row.title, "Ghost Town", "the filename is the one thing there is to show");
}

/// Everything the sparse row does *not* carry, asserted together because they share one reason:
/// the fields come from a `TrackListRow` that describes a different file, or no file, and drawing
/// any of them would state something about a track the library has never seen.
#[test]
fn a_sparse_row_states_nothing_it_cannot_know() {
    let file = BrowseFile {
        row: library_row(42),
        in_library: false,
    };

    let row = to_slint_browse_track_row(&file);

    assert_eq!(row.artist, "");
    assert_eq!(row.album, "");
    assert_eq!(row.genre, "");
    assert_eq!(row.artwork_path, "");
    assert_eq!(row.display_duration, "");
    assert_eq!(row.duration_ms, 0);
    assert_eq!(row.year, 0);
    assert_eq!(row.track_number, 0);
    assert_eq!(row.rating, 0);
    assert!(!row.is_favorite);
    assert_eq!((row.album_id, row.artist_id, row.genre_id), (0, 0, 0));
}
