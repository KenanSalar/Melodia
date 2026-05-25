use super::*;

use std::path::PathBuf;

// `RESPAWN_EXE` is a process-global static, so these assertions share one
// slot. They run inside a single `#[test]` to keep the set/read sequence
// deterministic — no other test in the crate touches the respawn slot.
#[test]
fn respawn_exe_round_trips_and_overwrites_the_recorded_path() {
    // Nothing recorded yet in a fresh test process.
    assert_eq!(respawn_exe(), None);

    // A recorded path round-trips verbatim.
    let first = PathBuf::from("/opt/melodia/bin/Melodia");
    set_respawn_exe(first.clone());
    assert_eq!(respawn_exe(), Some(first));

    // A later install in the same session overwrites the slot.
    let second = PathBuf::from("/usr/bin/Melodia");
    set_respawn_exe(second.clone());
    assert_eq!(respawn_exe(), Some(second));
}
