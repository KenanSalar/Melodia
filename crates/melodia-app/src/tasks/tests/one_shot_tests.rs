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

// --- A page's per-file read --------------------------------------------------

/// A read a test can fail without a codec: the file's text as a number.
fn number_in(path: &Path) -> AppResult<i64> {
    let text = std::fs::read_to_string(path)?;
    text.trim().parse().map_err(|e| AppError::validation("not a number", e))
}

fn row(id: i64, path: &Path) -> (i64, String) {
    (id, path.to_string_lossy().into_owned())
}

/// It fails the same way at every launch, so a sweep counting it would never retire.
#[test]
fn a_file_there_that_wont_read_is_left_out_and_counted_nowhere() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let (readable, unreadable) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::write(&readable, "7")?;
    std::fs::write(&unreadable, "seven")?;

    let reads = read_files(&[row(1, &readable), row(2, &unreadable)], number_in);

    assert_eq!((reads.read, reads.out_of_reach), (vec![(1, 7)], 0), "(read, out of reach)");
    Ok(())
}

/// An unmounted drive's files read next launch, which a sweep only gets to by failing this one.
#[test]
fn a_file_gone_from_disk_is_counted_out_of_reach() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;

    let reads = read_files(&[row(1, &tmp.path().join("gone"))], number_in);

    assert_eq!((reads.read, reads.out_of_reach), (Vec::new(), 1), "(read, out of reach)");
    Ok(())
}
