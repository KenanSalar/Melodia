use super::*;

/// Row whose `tag` is deliberately outside its `PartialEq`, so a test can
/// tell "the helper skipped this write" from "the helper wrote a value
/// that happened to look the same" — the distinction the whole
/// compare-and-skip fast path turns on.
#[derive(Clone, Debug)]
struct Row {
    id: i32,
    label: &'static str,
    tag: u32,
}

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.label == other.label
    }
}

fn row(id: i32, label: &'static str, tag: u32) -> Row {
    Row { id, label, tag }
}

fn tags(model: &VecModel<Row>) -> Vec<u32> {
    (0..model.row_count())
        .filter_map(|i| model.row_data(i).map(|r| r.tag))
        .collect()
}

fn labels(model: &VecModel<Row>) -> Vec<&'static str> {
    (0..model.row_count())
        .filter_map(|i| model.row_data(i).map(|r| r.label))
        .collect()
}

#[test]
fn equal_rows_are_not_written_back() {
    let model = VecModel::from(vec![row(1, "a", 0), row(2, "b", 0), row(3, "c", 0)]);

    apply_rows_keyed(
        &model,
        vec![row(1, "a", 9), row(2, "b", 9), row(3, "c", 9)],
        |r| r.id,
    );

    // Every incoming row compared equal, so none reached the model — the
    // original tags survive.
    assert_eq!(tags(&model), vec![0, 0, 0]);
}

#[test]
fn only_the_changed_row_is_written() {
    let model = VecModel::from(vec![row(1, "a", 0), row(2, "b", 0), row(3, "c", 0)]);

    apply_rows_keyed(
        &model,
        vec![row(1, "a", 9), row(2, "B", 9), row(3, "c", 9)],
        |r| r.id,
    );

    assert_eq!(labels(&model), vec!["a", "B", "c"]);
    assert_eq!(tags(&model), vec![0, 9, 0]);
}

#[test]
fn a_reorder_falls_back_to_a_full_reset() {
    let model = VecModel::from(vec![row(1, "a", 0), row(2, "b", 0)]);

    apply_rows_keyed(&model, vec![row(2, "b", 9), row(1, "a", 9)], |r| r.id);

    // Ids no longer align positionally, so the helper resets rather than
    // patching — which is what keeps a `ListView`'s delegate cache from
    // showing the old row under the new index.
    assert_eq!(tags(&model), vec![9, 9]);
    assert_eq!(labels(&model), vec!["b", "a"]);
}

#[test]
fn a_length_change_falls_back_to_a_full_reset() {
    let model = VecModel::from(vec![row(1, "a", 0), row(2, "b", 0)]);

    apply_rows_keyed(&model, vec![row(1, "a", 9)], |r| r.id);

    assert_eq!(tags(&model), vec![9]);
}

#[test]
fn an_empty_apply_clears_the_model() {
    let model = VecModel::from(vec![row(1, "a", 0)]);

    apply_rows_keyed(&model, Vec::new(), |r| r.id);

    assert_eq!(model.row_count(), 0);
}
