//! Which remembered decode failures may go stale.

use tempfile::TempDir;

use super::Decoded;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A cover reported missing has its references cleared, so a file that is merely unreadable
/// must never be taken for one.
#[test]
fn a_file_still_on_disk_that_will_not_decode_is_broken() -> TestResult {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("cover.jpg");
    std::fs::write(&path, b"not an image")?;

    assert!(matches!(Decoded::<()>::failed(&path), Decoded::Broken));
    Ok(())
}

#[test]
fn a_file_gone_from_disk_is_missing() -> TestResult {
    let tmp = TempDir::new()?;

    assert!(matches!(Decoded::<()>::failed(&tmp.path().join("cover.jpg")), Decoded::Missing));
    Ok(())
}

/// The restore brings a stored cover back under its old name, so only that answer can lapse. A
/// broken file would fail the same way again, and re-asking it would re-open it on every redraw.
#[test]
fn only_a_missing_answer_whose_file_is_back_goes_stale() -> TestResult {
    let tmp = TempDir::new()?;
    let present = tmp.path().join("present.jpg");
    std::fs::write(&present, b"a cover")?;
    let gone = tmp.path().join("gone.jpg");

    let cases = [
        ("ready, file gone", Decoded::Ready(()), &gone, true),
        ("broken, file present", Decoded::Broken, &present, true),
        ("missing, file still gone", Decoded::Missing, &gone, true),
        ("missing, file back", Decoded::Missing, &present, false),
    ];

    for (case, decoded, path, current) in cases {
        assert_eq!(decoded.is_current(path), current, "{case}");
    }
    Ok(())
}
