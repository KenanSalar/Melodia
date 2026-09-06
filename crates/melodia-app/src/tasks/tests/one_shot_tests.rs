//! The one knob, and the two real sweeps that set it opposite ways.
//!
//! `rating_import` is `Retry` and `artwork_renormalize` is `Mark`, and the difference is what a
//! failure costs: a renormalize that half-ran is repaired by the next scan, where an import that
//! failed is repaired by nothing and a marker over it hides every star in the library for the
//! life of the install. The two arms are one `matches!` apart and neither failure is visible.

use super::*;

fn sweep(on_failure: OnFailure) -> Sweep {
    Sweep {
        label: "Test sweep",
        marker: "test_sweep_done",
        done: |flags| flags.ratings_imported_from_tags,
        mark: |flags| flags.ratings_imported_from_tags = true,
        on_failure,
    }
}

/// The full table, because three of its four rows agree and the fourth is the whole feature: a
/// version answering `passed` alone re-runs every `Mark` sweep on every launch, and one answering
/// `true` alone retires a `Retry` sweep that never did its work.
#[test]
fn only_a_retry_sweep_that_failed_leaves_its_marker_down() {
    let rows: Vec<(&str, bool)> = vec![
        ("mark / passed", sweep(OnFailure::Mark).records_marker(true)),
        ("mark / failed", sweep(OnFailure::Mark).records_marker(false)),
        ("retry / passed", sweep(OnFailure::Retry).records_marker(true)),
        ("retry / failed", sweep(OnFailure::Retry).records_marker(false)),
    ];

    assert_eq!(
        rows,
        vec![
            ("mark / passed", true),
            ("mark / failed", true),
            ("retry / passed", true),
            ("retry / failed", false),
        ]
    );
}
