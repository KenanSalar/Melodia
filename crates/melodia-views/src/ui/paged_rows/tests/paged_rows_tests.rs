//! What a paged list asks the database for, what it tells Slint to read again, how a drag moves a
//! row, and the order its keyset loads show in.
//!
//! Two doubles stand in for what surrounds a list. The fetcher is the one the model takes as a
//! parameter, recording each request until a test lands it or drops it, which is a fetch failing.
//! A `slint::FilterModel` stands in for the `ListView`: it re-runs its predicate for exactly the row
//! a change names and for every row on a reset, so the ids it records are what a list would have
//! read back. A test reading a row itself is Slint drawing it.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use slint::{FilterModel, Model, ModelRc, SharedString};

use super::track_rows::TrackRows;
use super::{Keyset, KeysetTickets, Load, PagedRow, ReadStamp, RowRequest, Ticket, window};
use crate::ui::fixtures::track_keyset;
use melodia_ui::TrackListRow as UiTrackListRow;

/// What a landed row reads, where a placeholder's title is empty.
const LANDED: &str = "landed";

/// A list and every fetch it sends.
struct Fetching {
    rows: Rc<TrackRows>,
    /// The ids each fetch asked for, in the order sent.
    asked: Rc<RefCell<Vec<Vec<i64>>>>,
    /// The fetches neither landed nor dropped yet, oldest first.
    out: Rc<RefCell<VecDeque<RowRequest<UiTrackListRow>>>>,
}

impl Fetching {
    fn new() -> Self {
        let asked = Rc::new(RefCell::new(Vec::new()));
        let out = Rc::new(RefCell::new(VecDeque::new()));
        let (asked_log, out_queue) = (Rc::clone(&asked), Rc::clone(&out));
        let rows = TrackRows::new(move |request: RowRequest<UiTrackListRow>| {
            asked_log.borrow_mut().push(request.ids().to_vec());
            out_queue.borrow_mut().push_back(request);
        });
        Self { rows, asked, out }
    }

    /// A list over ids `1..=len` in that order, none of them fetched.
    fn over(len: i64) -> Self {
        let list = Self::new();
        list.rows.show(unseeded((1..=len).collect()));
        list
    }

    /// Slint drawing the row at `pos`.
    fn read(&self, pos: usize) {
        let _ = self.rows.row_data(pos);
    }

    fn asked(&self) -> Vec<Vec<i64>> {
        self.asked.borrow().clone()
    }

    /// The oldest fetch still out. Taken in its own statement, since landing one sends the next.
    fn take(&self) -> Option<RowRequest<UiTrackListRow>> {
        self.out.borrow_mut().pop_front()
    }

    /// Land the oldest fetch with a row for every id it asked for but `missing`.
    fn land_next_without(&self, missing: &[i64]) {
        let Some(request) = self.take() else { return };
        let rows = request
            .ids()
            .iter()
            .filter(|id| !missing.contains(id))
            .map(|&id| (id, landed(id)))
            .collect();
        request.land(rows);
    }

    /// Land the oldest fetch with `rows`, whatever it asked for.
    fn land_next_with(&self, rows: Vec<(i64, UiTrackListRow)>) {
        if let Some(request) = self.take() {
            request.land(rows);
        }
    }

    fn land_next(&self) {
        self.land_next_without(&[]);
    }

    /// The oldest fetch failing.
    fn drop_next(&self) {
        drop(self.take());
    }
}

fn unseeded(ids: Arc<[i64]>) -> Keyset<UiTrackListRow> {
    Keyset { ids, seed: Vec::new(), read: ReadStamp::now() }
}

fn landed(id: i64) -> UiTrackListRow {
    UiTrackListRow { title: SharedString::from(LANDED), ..UiTrackListRow::placeholder(id) }
}

/// [`track_keyset`] unfiltered in title order, 2 4 3 1, with `id`'s row rated `rating`.
fn rated(id: i64, rating: i32) -> Keyset<UiTrackListRow> {
    let mut keyset = track_keyset("", "asc");
    if let Some((_, row)) = keyset.seed.iter_mut().find(|(seeded, _)| *seeded == id) {
        row.rating = rating;
    }
    keyset
}

type Reread = Box<dyn Fn(&UiTrackListRow) -> bool>;

/// A list showing `fixtures::four_tracks` in title order, 2 4 3 1, every row seeded, and the ids of
/// every row read back since it was built.
struct Watched {
    rows: Rc<TrackRows>,
    list: FilterModel<ModelRc<UiTrackListRow>, Reread>,
    seen: Rc<RefCell<Vec<i32>>>,
}

impl Watched {
    fn new() -> Self {
        let rows = TrackRows::new(|_request| {});
        rows.show(track_keyset("", "asc"));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&seen);
        let reread: Reread = Box::new(move |row| {
            recorder.borrow_mut().push(row.id);
            true
        });
        let list = FilterModel::new(ModelRc::from(Rc::clone(&rows)), reread);
        seen.borrow_mut().clear();
        Self { rows, list, seen }
    }

    fn seen(&self) -> Vec<i32> {
        self.seen.borrow().clone()
    }
}

/// The ticket inside a load, whichever kind it runs as.
fn ticket(load: Load) -> Ticket {
    match load {
        Load::Requery(ticket) | Load::Reload(ticket) => ticket,
    }
}

// --- the window ---

/// What a list keeps fetched around the row read last, for the shipped 64-row page and two pages
/// either side. Its edges are where a scroll starts asking for rows, and a load seeds the same span
/// where the list is scrolled.
#[test]
fn the_window_is_the_page_read_and_two_either_side_within_the_list() {
    const CASES: [(usize, usize, Range<usize>, &str); 9] = [
        (0, 0, 0..0, "an empty list"),
        (1, 0, 0..1, "a one-row list"),
        (1000, 0, 0..192, "the first row of the first page"),
        (1000, 63, 0..192, "the last row of the first page"),
        (1000, 64, 0..256, "the first row of the second page"),
        (1000, 128, 0..320, "the third page"),
        (1000, 192, 64..384, "the fourth page, the first whose window leaves the top"),
        (1000, 999, 832..1000, "the last row, the window cut at the end"),
        (1000, 5000, 832..1000, "a row past the end after a narrowing, read as the last"),
    ];
    for (len, row, expected, case) in CASES {
        assert_eq!(window(len, row), expected, "{case}");
    }
}

// --- what a list fetches ---

/// A fast drag reads a page per frame, and a fetch per page queued enough reads that the pool
/// logged two-second waits.
#[test]
fn only_one_fetch_is_out_however_many_pages_are_read() {
    let list = Fetching::over(1000);
    list.read(0);

    list.read(500);

    assert_eq!(list.asked().len(), 1);
}

#[test]
fn a_landing_asks_next_for_the_rows_nearest_the_row_read_last() {
    let list = Fetching::over(1000);
    list.read(0);
    list.read(999);

    list.land_next();

    let nearest_the_end: Vec<i64> = (873..=1000).rev().collect();
    assert_eq!(list.asked().get(1), Some(&nearest_the_end));
}

/// Row 500 sits at id 501. The drag passed it while the first fetch was out, and stopped at the
/// end, which is all the next fetch is about.
#[test]
fn a_page_scrolled_past_while_a_fetch_was_out_is_never_asked_for() {
    let list = Fetching::over(1000);
    list.read(0);
    list.read(500);
    list.read(999);

    list.land_next();

    assert!(!list.asked().concat().contains(&501));
}

/// A row deleted after the keyset was read comes back from no fetch, and reading its placeholder
/// again must not send one that would come back short the same way, forever.
#[test]
fn a_row_a_fetch_did_not_return_is_not_asked_for_again() {
    let list = Fetching::over(4);
    list.read(0);
    list.land_next_without(&[3]);

    list.read(2);

    assert_eq!(list.asked().len(), 1);
}

/// The library tick that deleted the row brings a keyset without it; until then a new keyset is
/// the only thing that may say the row could be back.
#[test]
fn a_new_keyset_asks_again_for_a_row_an_earlier_fetch_missed() {
    let list = Fetching::over(4);
    list.read(0);
    list.land_next_without(&[3]);
    list.rows.show(unseeded((1..=4).collect()));

    list.read(2);

    assert_eq!(list.asked().last(), Some(&vec![3]));
}

/// A database refusing every fetch costs one per scroll rather than one per landing.
#[test]
fn a_failed_fetch_sends_nothing_until_the_list_is_read_again() {
    let list = Fetching::over(1000);
    list.read(0);

    list.drop_next();

    assert_eq!(list.asked().len(), 1);
}

#[test]
fn a_read_after_a_failed_fetch_sends_a_new_one() {
    let list = Fetching::over(1000);
    list.read(0);
    list.drop_next();

    list.read(0);

    assert_eq!(list.asked().len(), 2);
}

/// A section leave clears the list; rows a fetch sent before it brings back would sit in the cache
/// of a list nobody can see.
#[test]
fn a_fetch_landing_after_a_clear_stores_nothing() {
    let list = Fetching::over(4);
    list.read(0);
    list.rows.clear();

    list.land_next();

    list.rows.show(unseeded((1..=4).collect()));
    let title = list.rows.row_data(0).map(|row| row.title);
    assert_eq!(title, Some(SharedString::new()));
}

#[test]
fn an_empty_list_reads_no_row_and_sends_no_fetch() {
    let list = Fetching::over(0);

    let first = list.rows.row_data(0);

    assert_eq!((list.rows.row_count(), first.map(|row| row.id), list.asked().len()), (0, None, 0));
}

/// A row reads its neighbours' flags to join a run of picks, and those may not have landed.
#[test]
fn a_row_not_fetched_yet_still_carries_its_selection_flag() {
    let list = Fetching::over(1000);
    list.rows.select(&HashSet::from([500]));

    let row = list.rows.row_data(499);

    assert_eq!(row.map(|row| (row.title.is_empty(), row.selected)), Some((true, true)));
}

/// What a load running off the event loop seeds its rows around.
#[test]
fn the_scroll_point_follows_the_row_read_last() {
    let list = Fetching::over(1000);

    list.read(700);

    assert_eq!(list.rows.scroll_point().row(), 700);
}

#[test]
fn a_clear_sends_the_scroll_point_back_to_the_top() {
    let list = Fetching::over(1000);
    list.read(700);

    list.rows.clear();

    assert_eq!(list.rows.scroll_point().row(), 0);
}

// --- what a list tells Slint to read again ---

/// The only way a heart or a star change reaches the screen on either Songs list.
#[test]
fn a_patch_rereads_only_the_row_showing_that_id() {
    let watched = Watched::new();

    watched.rows.patch(3, |row| row.rating = 5);

    assert_eq!(watched.seen(), [3]);
}

/// A heart clicked on a row the cache has evicted: its next fetch reads the write.
#[test]
fn a_patch_of_a_row_not_cached_rereads_nothing() {
    let watched = Watched::new();

    watched.rows.patch(99, |row| row.rating = 5);

    assert_eq!(watched.seen(), Vec::<i32>::new());
}

/// Every library tick reloads the list, and a reset rebuilds every `ListView` delegate for rows
/// that read the same.
#[test]
fn showing_the_same_rows_again_rereads_nothing() {
    let watched = Watched::new();

    watched.rows.show(track_keyset("", "asc"));

    assert_eq!(watched.seen(), Vec::<i32>::new());
}

/// A rating changed elsewhere arrives only through a reload, which compares the rows as well as
/// the ids.
#[test]
fn showing_the_same_ids_with_one_row_changed_rereads_only_that_row() {
    let watched = Watched::new();

    watched.rows.show(rated(3, 5));

    assert_eq!(watched.seen(), [3]);
}

/// A star clicked while a re-read is out: the fetch was sent first and lands last, carrying the
/// rating from before the click. Track 3 sits third in title order, and the one fetch counted is
/// the re-read's, without which nothing landed late at all.
#[test]
fn a_fetch_sent_before_a_patch_never_reverts_it() {
    let list = Fetching::new();
    let shown = track_keyset("", "asc");
    let ids = Arc::clone(&shown.ids);
    list.rows.show(shown);
    list.read(0);
    list.rows.show(unseeded(ids));
    list.rows.reread();
    list.rows.patch(3, |row| row.rating = 5);

    list.land_next_with(track_keyset("", "asc").seed);

    let third = list.rows.row_data(2).map(|row| (row.id, row.rating));
    assert_eq!((list.asked().len(), third), (1, Some((3, 5))));
}

/// An unfavourite on Favorites ▸ Songs drops its row on the click; every other row stays where it
/// is on screen.
#[test]
fn removing_one_row_shortens_the_list_without_rereading_the_rest() {
    let watched = Watched::new();

    watched.rows.remove(&HashSet::from([4]));

    assert_eq!((watched.list.row_count(), watched.seen()), (3, vec![]));
}

/// Title order 2 4 3 1 without 4 and 1 leaves 2 then 3.
#[test]
fn removing_two_rows_resets_the_list() {
    let watched = Watched::new();

    watched.rows.remove(&HashSet::from([4, 1]));

    assert_eq!((watched.list.row_count(), watched.seen()), (2, vec![2, 3]));
}

// --- a playlist drag ---

/// `to` is counted once the row has left, which is the index a drop hands over.
#[test]
fn a_move_lands_the_row_where_the_drag_drops_it() {
    const CASES: [(usize, usize, [i64; 4], &str); 3] = [
        (0, 3, [4, 3, 1, 2], "the top row to the slot after the last"),
        (3, 0, [1, 2, 4, 3], "the last row to the top"),
        (1, 2, [2, 3, 4, 1], "one row down"),
    ];
    for (from, to, expected, case) in CASES {
        let watched = Watched::new();

        let moved = watched.rows.move_row(from, to);

        assert_eq!((moved, &watched.rows.ids()[..]), (true, &expected[..]), "{case}");
    }
}

/// One row leaves and one arrives, so every other row keeps its `ListView` delegate.
#[test]
fn a_move_rereads_only_the_row_it_moved() {
    let watched = Watched::new();

    watched.rows.move_row(0, 3);

    assert_eq!((watched.list.row_count(), watched.seen()), (4, vec![2]));
}

#[test]
fn a_move_from_or_to_outside_the_list_moves_nothing() {
    const CASES: [(usize, usize, &str); 2] =
        [(4, 0, "a source one past the last row"), (0, 5, "a drop past the end of the list")];
    for (from, to, case) in CASES {
        let watched = Watched::new();

        let moved = watched.rows.move_row(from, to);

        let unmoved = (false, &[2, 4, 3, 1][..], Vec::<i32>::new());
        assert_eq!((moved, &watched.rows.ids()[..], watched.seen()), unmoved, "{case}");
    }
}

#[test]
fn an_empty_list_moves_nothing() {
    let list = Fetching::over(0);

    let moved = list.rows.move_row(0, 0);

    assert!(!moved);
}

// --- the order loads show in ---

#[test]
fn only_the_newest_ticket_is_latest() {
    let tickets = KeysetTickets::default();
    let older = tickets.issue_reload();

    let newer = tickets.issue_reload();

    assert_eq!((tickets.is_latest(older), tickets.is_latest(newer)), (false, true));
}

#[test]
fn a_requery_with_no_reload_owed_runs_as_a_requery() {
    let tickets = KeysetTickets::default();

    let load = tickets.issue_requery();

    assert!(matches!(load, Load::Requery(_)), "{load:?}");
}

/// A header click right after a heart: the click's load is the one shown, so it has to re-read
/// the rows the heart's library tick would have.
#[test]
fn a_requery_overtaking_a_reload_runs_as_one() {
    let tickets = KeysetTickets::default();
    let _ = tickets.issue_reload();

    let load = tickets.issue_requery();

    assert!(matches!(load, Load::Reload(_)), "{load:?}");
}

#[test]
fn a_reload_that_showed_is_owed_no_more() {
    let tickets = KeysetTickets::default();
    let reload = tickets.issue_reload();
    tickets.reload_shown(reload);

    let load = tickets.issue_requery();

    assert!(matches!(load, Load::Requery(_)), "{load:?}");
}

#[test]
fn a_requery_standing_in_for_a_reload_settles_it_when_shown() {
    let tickets = KeysetTickets::default();
    let _ = tickets.issue_reload();
    let stand_in = ticket(tickets.issue_requery());
    tickets.reload_shown(stand_in);

    let load = tickets.issue_requery();

    assert!(matches!(load, Load::Requery(_)), "{load:?}");
}

/// The older reload lands first, but a second library change came after it asked.
#[test]
fn an_older_reload_showing_leaves_a_newer_one_owed() {
    let tickets = KeysetTickets::default();
    let older = tickets.issue_reload();
    let _ = tickets.issue_reload();
    tickets.reload_shown(older);

    let load = tickets.issue_requery();

    assert!(matches!(load, Load::Reload(_)), "{load:?}");
}

/// An open overtaken by a keystroke on the page it leaves asks again before that keystroke's load
/// lands, which is then refused, so the reload the open owed rides on the second requery.
#[test]
fn a_requery_overtaking_a_stand_in_still_runs_as_the_reload() {
    let tickets = KeysetTickets::default();
    let _ = tickets.issue_reload();
    let _ = tickets.issue_requery();

    let load = tickets.issue_requery();

    assert!(matches!(load, Load::Reload(_)), "{load:?}");
}

/// A close or a section leave empties the list a load in flight would land in.
#[test]
fn a_revoke_refuses_the_newest_load() {
    let tickets = KeysetTickets::default();
    let reload = tickets.issue_reload();

    tickets.revoke();

    assert!(!tickets.is_latest(reload));
}

/// The next load seats a fresh keyset, so a released list owes no re-read.
#[test]
fn a_revoke_leaves_no_reload_owed() {
    let tickets = KeysetTickets::default();
    let _ = tickets.issue_reload();
    tickets.revoke();

    let load = tickets.issue_requery();

    assert!(matches!(load, Load::Requery(_)), "{load:?}");
}

/// A detail reopened after a release shows its load.
#[test]
fn a_load_asked_for_after_a_revoke_is_the_latest() {
    let tickets = KeysetTickets::default();
    tickets.revoke();

    let reload = tickets.issue_reload();

    assert!(tickets.is_latest(reload));
}
