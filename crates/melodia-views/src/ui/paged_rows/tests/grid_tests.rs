//! What a card grid hands Slint: how many chunk rows, which of them are the model Slint already
//! holds, and which cell a card that lands repaints.
//!
//! The fetcher records each request until a test lands it. A `slint::FilterModel` over one chunk
//! stands in for the row's repeater: it re-runs its predicate for exactly the card a change names,
//! and for every card on a reset.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use slint::{FilterModel, Model, SharedString};

use super::PagedGrid;
use crate::ui::paged_rows::{Keyset, PagedRow, ReadStamp, RowRequest};
use melodia_ui::{AlbumGridRow as UiAlbumGridRow, AlbumRow as UiAlbumRow};

/// What a landed card reads, where a placeholder's name is empty.
const LANDED: &str = "landed";

struct Grid {
    grid: Rc<PagedGrid<UiAlbumGridRow>>,
    /// The fetches not landed yet, oldest first.
    out: Rc<RefCell<VecDeque<RowRequest<UiAlbumRow>>>>,
}

impl Grid {
    /// A grid `columns` wide over ids `1..=len`, with the cards for `seeded` already fetched.
    fn new(columns: i32, len: i64, seeded: &[i64]) -> Self {
        let out = Rc::new(RefCell::new(VecDeque::new()));
        let queue = Rc::clone(&out);
        let grid = PagedGrid::new(move |request| queue.borrow_mut().push_back(request));
        grid.set_columns(columns);
        grid.show(Keyset {
            ids: (1..=len).collect::<Arc<[i64]>>(),
            seed: seeded.iter().map(|&id| (id, landed(id))).collect(),
            read: ReadStamp::now(),
        });
        Self { grid, out }
    }

    /// Slint building grid row `row`.
    fn row(&self, row: usize) -> Option<UiAlbumGridRow> {
        self.grid.row_data(row)
    }

    /// The oldest fetch landing with a card for every id it asked for.
    fn land_next(&self) {
        let Some(request) = self.out.borrow_mut().pop_front() else { return };
        let cards = request.ids().iter().map(|&id| (id, landed(id))).collect();
        request.land(cards);
    }
}

fn landed(id: i64) -> UiAlbumRow {
    UiAlbumRow { name: SharedString::from(LANDED), ..UiAlbumRow::placeholder(id) }
}

/// The cards of `row` as Slint draws them.
fn cards(row: Option<&UiAlbumGridRow>) -> Vec<(i32, SharedString)> {
    row.map(|row| row.albums.iter().map(|card| (card.id, card.name)).collect()).unwrap_or_default()
}

/// How many cards the grid's last row draws, `0` where it has none.
fn last_row_len(grid: &Grid) -> usize {
    grid.grid
        .row_count()
        .checked_sub(1)
        .and_then(|last| grid.row(last))
        .map_or(0, |row| row.albums.row_count())
}

#[test]
fn a_grid_has_one_row_per_column_count_of_cards_and_a_short_last_one() {
    const CASES: [(i64, i32, usize, usize, &str); 5] = [
        (0, 3, 0, 0, "an empty grid"),
        (6, 3, 2, 3, "a count the columns divide"),
        (7, 3, 3, 1, "one card past a full row"),
        (2, 3, 1, 2, "fewer cards than one row holds"),
        (3, 0, 3, 1, "a grid mid-layout reporting no columns, floored at one"),
    ];
    for (len, columns, rows, last, case) in CASES {
        let grid = Grid::new(columns, len, &[]);

        assert_eq!((grid.grid.row_count(), last_row_len(&grid)), (rows, last), "{case}");
    }
}

/// A grid row's repeater compares models by pointer and rebuilds every card of a row whose model
/// is new, so a chunk dropped from the memo while Slint still draws it stutters a scroll.
#[test]
fn a_chunk_slint_still_holds_is_handed_back_while_another_is_built() {
    let grid = Grid::new(3, 9, &[]);
    let held = grid.row(0);
    let _built = grid.row(2);

    let again = grid.row(0);

    assert_eq!(again, held, "the same row model, compared by pointer");
}

#[test]
fn a_card_that_lands_repaints_its_own_cell_alone() {
    let grid = Grid::new(3, 6, &[1, 2, 3, 4, 6]);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let recorder = Rc::clone(&seen);
    // A missing row reads as an empty one, which the assertion below names.
    let chunk = grid.row(1).map(|row| row.albums).unwrap_or_default();
    let _repeater = FilterModel::new(chunk, move |card: &UiAlbumRow| {
        recorder.borrow_mut().push((card.id, card.name.clone()));
        true
    });
    seen.borrow_mut().clear();

    grid.land_next();

    assert_eq!(*seen.borrow(), [(5, SharedString::from(LANDED))]);
}

/// The column count arrives with every load, so the guard is what keeps a reload that moved no
/// card from rebuilding them all.
#[test]
fn setting_the_column_count_the_grid_already_has_rebuilds_nothing() {
    let grid = Grid::new(3, 6, &[]);
    let held = grid.row(0);

    grid.grid.set_columns(3);

    assert_eq!(grid.row(0), held, "the same row model, compared by pointer");
}

#[test]
fn a_column_change_rechunks_the_rows_slint_holds() {
    let grid = Grid::new(3, 6, &[]);
    let _held = grid.row(0);

    grid.grid.set_columns(2);

    let first_row: Vec<i32> = cards(grid.row(0).as_ref()).into_iter().map(|(id, _)| id).collect();
    assert_eq!(first_row, [1, 2]);
}

/// Every card is seeded and none is ever landed, so a column change that dropped the cache would
/// read placeholders back.
#[test]
fn a_column_change_keeps_every_card_it_fetched() {
    let grid = Grid::new(3, 6, &[1, 2, 3, 4, 5, 6]);

    grid.grid.set_columns(2);

    let names: Vec<SharedString> =
        (0..3).flat_map(|row| cards(grid.row(row).as_ref())).map(|(_, name)| name).collect();
    assert_eq!(names, [LANDED; 6]);
}
