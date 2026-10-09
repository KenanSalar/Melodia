//! What a completed scan purges.

use std::path::{Path, PathBuf};

use super::orphans_of;
use crate::media::ingest::scanner::MediaWalk;

fn row(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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
