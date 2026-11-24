use std::io::{Read, Seek, SeekFrom};

use tempfile::TempDir;

use super::*;
use melodia_core::error::AppError;

/// Ten bytes, `0` to `9`, read as though the file ended after the seventh.
fn bounded_at_seven(tmp: &TempDir) -> Result<FileSource, AppError> {
    let path = tmp.path().join("track");
    std::fs::write(&path, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9])?;
    Ok(FileSource::ending_at(File::open(&path)?, 7))
}

#[test]
fn a_bounded_source_reads_nothing_past_its_end() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let mut source = bounded_at_seven(&tmp)?;
    source.seek(SeekFrom::Start(5))?;

    let mut read = Vec::new();
    source.read_to_end(&mut read)?;

    assert_eq!(read, [5, 6]);
    Ok(())
}

/// A demuxer looks for a stream's last page or frame by seeking from the end, which has to mean
/// the bound, or it lands among the tags.
#[test]
fn a_bounded_source_counts_a_seek_from_the_end_from_its_bound() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let mut source = bounded_at_seven(&tmp)?;

    let landed = source.seek(SeekFrom::End(-2))?;

    assert_eq!(landed, 5);
    Ok(())
}
