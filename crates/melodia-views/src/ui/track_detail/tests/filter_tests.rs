use std::cell::Cell;
use std::rc::Rc;

use slint::{Model, ModelRc, VecModel};

use super::apply_filtered_detail;
use crate::ui::fixtures::four_tracks;
use crate::ui::list_selection::{NO_ANCHOR, RowSelectionView};
use crate::ui::track_detail::{DetailCache, selection};
use melodia_ui::TrackListRow as UiTrackListRow;

/// What a detail global holds of its selection, over the `VecModel` every detail installs.
struct FakeDetail {
    anchor: Cell<i32>,
    selected: Rc<VecModel<i32>>,
    rows: Rc<VecModel<UiTrackListRow>>,
}

impl FakeDetail {
    /// A detail over [`four_tracks`] in fetch order, 1 2 3 4, with nothing selected.
    fn opened(cache: &DetailCache) -> Self {
        let view = Self {
            anchor: Cell::new(NO_ANCHOR),
            selected: Rc::new(VecModel::default()),
            rows: Rc::new(VecModel::default()),
        };
        cache.seat_unfiltered(four_tracks());
        apply_filtered_detail(&view, cache);
        view
    }

    /// `(selected-ids, the anchor, each row's id and flag)`.
    fn state(&self) -> (Vec<i32>, i32, Vec<(i32, bool)>) {
        let rows = self.rows.iter().map(|row| (row.id, row.selected)).collect();
        (self.selected.iter().collect(), self.anchor.get(), rows)
    }
}

impl RowSelectionView for FakeDetail {
    fn anchor(&self) -> i32 {
        self.anchor.get()
    }
    fn set_anchor(&self, anchor: i32) {
        self.anchor.set(anchor);
    }
    fn selected_ids(&self) -> ModelRc<i32> {
        ModelRc::from(Rc::clone(&self.selected))
    }
    fn replace_selected_ids(&self, _ids: ModelRc<i32>) {}
    fn track_rows(&self) -> ModelRc<UiTrackListRow> {
        ModelRc::from(Rc::clone(&self.rows))
    }
    fn replace_track_rows(&self, _rows: ModelRc<UiTrackListRow>) {}
    fn bind_select_row(&self, _f: impl FnMut(i32, i32, bool, bool) + 'static) {}
    fn bind_select_all(&self, _f: impl FnMut() + 'static) {}
    fn bind_clear_selection(&self, _f: impl FnMut() + 'static) {}
}

/// The needle hides track 1 and leaves it in the entity, so widening it again paints the pick.
#[test]
fn a_refilter_keeps_a_selection_the_needle_hides() {
    let cache = DetailCache::default();
    let view = FakeDetail::opened(&cache);
    selection::handle_select_row(&view, &cache, 0, 1, false, false);
    cache.set_filter("ha");

    apply_filtered_detail(&view, &cache);

    assert_eq!(view.state(), (vec![1], 1, vec![(2, false), (3, false)]));
}

/// A refresh after track 3 left the entity, the anchor going with it.
#[test]
fn a_refresh_drops_a_track_the_entity_no_longer_holds_from_the_selection() {
    let cache = DetailCache::default();
    let view = FakeDetail::opened(&cache);
    selection::handle_select_row(&view, &cache, 1, 2, false, false);
    selection::handle_select_row(&view, &cache, 2, 3, false, true);
    cache.set_all_tracks(four_tracks().into_iter().filter(|track| track.id != 3).collect());

    apply_filtered_detail(&view, &cache);

    assert_eq!(view.state(), (vec![2], NO_ANCHOR, vec![(1, false), (2, true), (4, false)]));
}
