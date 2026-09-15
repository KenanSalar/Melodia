use super::{nul_terminated_wide, read_user_dword};

/// `RegGetValueW` reads a name up to its NUL, so a buffer without one runs on into whatever memory
/// follows it.
#[test]
fn a_name_reaches_the_api_nul_terminated() {
    assert_eq!(nul_terminated_wide("Ab"), [u16::from(b'A'), u16::from(b'b'), 0]);
}

/// The `None` both callers fail open on.
#[test]
fn a_missing_key_reads_as_none() {
    assert_eq!(read_user_dword("Software\\Melodia-tests\\no-such-key", "no-such-value"), None);
}
