use melodia_core::entities::browse::BrowseFile;

use super::row_ids;
use crate::ui::fixtures::track_row;

fn file(id: i64, in_library: bool) -> BrowseFile {
    BrowseFile { row: track_row(id, "Ghost Town"), file_name: "ghost.flac".to_owned(), in_library }
}

/// Dropping the disk-only row would shift every index after it onto its neighbour, and keeping
/// its id would hand a range a track the library does not hold.
#[test]
fn a_disk_only_row_keeps_its_place_without_its_id() {
    let files = [file(10, true), file(42, false), file(20, true)];

    assert_eq!(row_ids(&files), [10, 0, 20]);
}
