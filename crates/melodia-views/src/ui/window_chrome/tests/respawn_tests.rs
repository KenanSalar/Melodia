use super::*;

use std::path::PathBuf;

// `RESPAWN_EXE` is a process-global static, so these assertions share one
// slot. They run inside a single `#[test]` to keep the set/read sequence
// deterministic — no other test in the crate touches the respawn slot.
#[test]
fn respawn_exe_round_trips_and_overwrites_the_recorded_path() {
    // Nothing recorded yet in a fresh test process, so the target falls
    // through to whatever the OS says this binary is — never `None`, which
    // is the answer that would keep a restart from happening at all.
    assert_eq!(respawn_exe(), None);
    assert!(respawn_target().is_some());

    // A recorded path round-trips verbatim, and outranks the fallback: the
    // updater records it precisely because the OS answer has gone stale by
    // the time the respawn runs.
    let first = PathBuf::from("/opt/melodia/bin/Melodia");
    set_respawn_exe(first.clone());
    assert_eq!(respawn_exe(), Some(first.clone()));
    assert_eq!(respawn_target(), Some(first));

    // A later install in the same session overwrites the slot.
    let second = PathBuf::from("/usr/bin/Melodia");
    set_respawn_exe(second.clone());
    assert_eq!(respawn_exe(), Some(second.clone()));
    assert_eq!(respawn_target(), Some(second));
}
