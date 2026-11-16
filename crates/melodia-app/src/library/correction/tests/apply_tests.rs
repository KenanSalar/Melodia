//! What Import reads before the review: one size cap for a file and a paste, and a file read
//! leniently, since a profile's comments are whatever its author's editor wrote.

use std::path::{Path, PathBuf};

use super::{IMPORT_MAX_BYTES, ImportError, Imported, import_file, import_paste};
use melodia_core::entities::correction::CorrectionOrigin;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FILTER_LINE: &str = "Filter 1: ON PK Fc 105 Hz Gain 6.5 dB Q 0.70";

/// One filter padded out with a comment to exactly `len` bytes.
fn profile_of_len(len: usize) -> String {
    let header = format!("{FILTER_LINE}\n# ");
    format!("{header}{}", "x".repeat(len - header.len()))
}

/// How an import ended, as the dialog tells them apart.
fn outcome(read: &Result<Imported, ImportError>) -> &'static str {
    match read {
        Ok(_) => "read",
        Err(ImportError::TooLarge) => "too large",
        Err(ImportError::Refused(_)) => "refused",
        Err(ImportError::Read(_)) => "unread",
    }
}

fn written(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

#[test]
fn a_paste_is_held_to_the_import_cap() -> TestResult {
    let cap = usize::try_from(IMPORT_MAX_BYTES)?;

    let cases = [(cap, "read"), (cap + 1, "too large")];
    for (len, expected) in cases {
        assert_eq!(outcome(&import_paste(&profile_of_len(len))), expected, "{len} bytes");
    }
    Ok(())
}

#[test]
fn a_file_is_held_to_the_import_cap() -> TestResult {
    let cap = usize::try_from(IMPORT_MAX_BYTES)?;
    let dir = tempfile::tempdir()?;

    let cases = [(cap, "read"), (cap + 1, "too large")];
    for (len, expected) in cases {
        let path = written(dir.path(), "profile.txt", profile_of_len(len).as_bytes())?;

        assert_eq!(outcome(&import_file(&path)), expected, "{len} bytes");
    }
    Ok(())
}

/// A comment saved in Latin-1 is not UTF-8, and refusing the file over it would refuse filters
/// that read perfectly well.
#[test]
fn a_file_with_a_comment_in_another_codepage_is_read() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut bytes = b"# \xE9couteurs\n".to_vec();
    bytes.extend_from_slice(FILTER_LINE.as_bytes());
    let path = written(dir.path(), "latin1.txt", &bytes)?;

    assert_eq!(outcome(&import_file(&path)), "read");
    Ok(())
}

/// The card names a file's correction by the file, never by where it sat on disk.
#[test]
fn a_file_import_is_named_by_its_file_alone() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = written(dir.path(), "DT 990 Pro ParametricEQ.txt", FILTER_LINE.as_bytes())?;

    let read = import_file(&path);

    let name = match read.map(|imported| imported.origin) {
        Ok(CorrectionOrigin::File { name }) => Some(name),
        _ => None,
    };
    assert_eq!(name.as_deref(), Some("DT 990 Pro ParametricEQ.txt"));
    Ok(())
}
