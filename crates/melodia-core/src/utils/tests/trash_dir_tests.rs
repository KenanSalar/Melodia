//! Which folders count as a file manager's trash.
//!
//! The home trash, matched by path, goes untested here: its path is resolved once per process from
//! the environment, which another test in this binary rewrites.

use std::path::{Path, PathBuf};

use super::*;

fn in_library(names: &[&str]) -> PathBuf {
    names.iter().fold(Path::new("library").to_path_buf(), |path, name| path.join(name))
}

#[test]
fn a_volume_trash_is_known_by_its_name() {
    let names = [".Trash", ".Trash-1000", ".Trashes", "$RECYCLE.BIN", "$Recycle.Bin"];
    for name in names {
        assert!(is_trash_dir(&in_library(&[name])), "{name}");
    }
}

/// Each would cost a library folder the music under it, the walk pruning everything below.
#[test]
fn a_name_that_only_resembles_a_trash_is_walked() {
    let names = ["Trash", ".trash", ".Trash-", ".Trash-1000a", ".Trash-abc", ".Trashcan"];
    for name in names {
        assert!(!is_trash_dir(&in_library(&["Album", name])), "{name}");
    }
}

#[test]
fn a_file_is_in_a_trash_when_any_folder_above_it_is_one() {
    let rows = [
        (in_library(&[".Trash-1000", "files", "song.mp3"]), true),
        (in_library(&["$RECYCLE.BIN", "song.mp3"]), true),
        (in_library(&["Album", "song.mp3"]), false),
        (in_library(&["Album", ".Trash"]), false),
    ];
    for (path, expected) in rows {
        assert_eq!(in_trash_dir(&path), expected, "{}", path.display());
    }
}
