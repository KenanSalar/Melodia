//! What a completed scan purges, and what a scan's chunks add up to.

use std::path::{Path, PathBuf};

use super::{Ingested, orphans_of};
use melodia_store::database::queries::ingest::IngestResult;
use melodia_store::media::ingest::scanner::MediaWalk;

fn row(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn chunk(inserted: u32, moved: u32, updated: u32) -> IngestResult {
    IngestResult {
        inserted_count: inserted,
        moved_count: moved,
        updated_count: updated,
        inserted_track_ids: Vec::new(),
    }
}

/// The incremental filter leaves an unchanged file unread, so a row is checked against the walk.
/// Checked against what was parsed, a rescan with nothing new would purge the whole library.
#[test]
fn only_a_row_the_walk_did_not_find_is_an_orphan() {
    let root = PathBuf::from("music");
    let found = root.join("found.mp3");
    let gone = root.join("gone.mp3");
    let walk = MediaWalk { files: vec![found.clone()], unreadable: Vec::new() };

    let orphans = orphans_of(vec![row(&found), row(&gone)], walk);

    assert_eq!(orphans, [row(&gone)]);
}

/// A directory the walk couldn't read says nothing about the files in it, and a row purged there
/// takes its rating and play count with it.
#[test]
fn a_row_under_a_path_the_walk_could_not_read_is_spared() {
    let root = PathBuf::from("music");
    let unreadable = root.join("locked");
    let inside = unreadable.join("a.mp3");
    let gone = root.join("gone.mp3");
    let walk = MediaWalk { files: Vec::new(), unreadable: vec![unreadable] };

    let orphans = orphans_of(vec![row(&inside), row(&gone)], walk);

    assert_eq!(orphans, [row(&gone)]);
}

/// `any` is what a stopped scan still owes the roll-ups and the recalc for, and
/// `rewrote_existing` is what keeps a cancelled import from being withdrawn with tracks the
/// library held before it.
#[test]
fn what_the_chunks_wrote_decides_the_last_write_and_the_withdraw() {
    let cases = [
        // (inserted, moved, updated), then (any, rewrote_existing)
        ((0, 0, 0), (false, false)),
        ((3, 0, 0), (true, false)),
        ((0, 1, 0), (true, true)),
        ((0, 0, 1), (true, true)),
    ];

    for ((inserted, moved, updated), expected) in cases {
        let mut ingested = Ingested::new(false);
        ingested.add(&chunk(inserted, moved, updated));
        let decided = (ingested.any(), ingested.rewrote_existing());
        assert_eq!(decided, expected, "inserted {inserted}, moved {moved}, updated {updated}");
    }
}

/// A rewrite in an early chunk must still keep the import, however many chunks after it only
/// inserted.
#[test]
fn the_counts_sum_across_chunks() {
    let mut ingested = Ingested::new(true);

    ingested.add(&chunk(2, 1, 0));
    ingested.add(&chunk(3, 0, 0));

    assert_eq!((ingested.inserted, ingested.rewrote_existing()), (5, true));
}
