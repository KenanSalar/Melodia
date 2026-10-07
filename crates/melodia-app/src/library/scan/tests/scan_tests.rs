//! Which files under a folder its own scan reads.

use std::path::PathBuf;

use super::ScanScope;
use crate::library::settings::folders::NestedFolder;

/// A library folder nested inside this one is scanned by its own scan until a completed one here
/// absorbs it. The folder an import filed loose tracks under is scanned by nothing, so its files
/// are read here. Compared by component: a sibling sharing a nested folder's name as a prefix is
/// this scan's, or nothing would read it.
#[test]
fn a_scan_reads_everything_under_it_but_a_nested_library_folder() {
    let root = PathBuf::from("music");
    let scope = ScanScope {
        root: root.clone(),
        nested: vec![
            NestedFolder { id: 2, path: root.join("rock"), is_enabled: true },
            NestedFolder { id: 3, path: root.join("imported"), is_enabled: false },
        ],
    };
    let cases = [
        (root.join("a.mp3"), true),
        (root.join("rock").join("b.mp3"), false),
        (root.join("imported").join("c.mp3"), true),
        (root.join("rockabilly").join("d.mp3"), true),
    ];

    for (path, owned) in cases {
        assert_eq!(scope.owns(&path), owned, "{}", path.display());
    }
}
