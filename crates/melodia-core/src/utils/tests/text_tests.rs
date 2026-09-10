//! The one "is there anything here" rule two crates share.

use super::filled;

/// A directory serves `""` about as readily as it omits a field, and a scan can leave a run of
/// spaces in an artist tag. Both mean nothing is there.
#[test]
fn a_field_holding_only_space_reads_as_absent() {
    assert_eq!(filled(None), None);
    assert_eq!(filled(Some("")), None);
    assert_eq!(filled(Some("   ")), None);
    assert_eq!(filled(Some("\t\n")), None);
}

#[test]
fn a_field_with_text_comes_back_trimmed() {
    assert_eq!(filled(Some("Alice")), Some("Alice"));
    assert_eq!(filled(Some("  Alice  ")), Some("Alice"));
}
