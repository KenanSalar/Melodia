use super::*;

fn paths(of: &[Option<&str>], cap: usize) -> Vec<String> {
    unique_artwork_paths(of.iter().copied(), cap)
        .iter()
        .filter_map(|p| p.to_str().map(str::to_owned))
        .collect()
}

#[test]
fn duplicates_collapse_and_first_seen_order_survives() {
    let out = paths(
        &[Some("b.jpg"), Some("a.jpg"), Some("b.jpg"), Some("c.jpg")],
        16,
    );
    assert_eq!(out, vec!["b.jpg", "a.jpg", "c.jpg"]);
}

#[test]
fn missing_and_empty_paths_are_skipped() {
    let out = paths(&[None, Some(""), Some("a.jpg"), None, Some("")], 16);
    assert_eq!(out, vec!["a.jpg"]);
}

#[test]
fn the_cap_counts_kept_paths_not_input_items() {
    // Five inputs, three of which are duplicates of the first: a cap of 2
    // has to yield two *distinct* covers, not stop after the second input.
    let out = paths(
        &[
            Some("a.jpg"),
            Some("a.jpg"),
            Some("a.jpg"),
            Some("b.jpg"),
            Some("c.jpg"),
        ],
        2,
    );
    assert_eq!(out, vec!["a.jpg", "b.jpg"]);
}

#[test]
fn a_zero_cap_yields_nothing() {
    assert!(paths(&[Some("a.jpg")], 0).is_empty());
}
