//! The one thing in the shutdown sequence a test can reach: which restart arm marks its child.
//!
//! The rest of the file needs an `AppWindow`, an `AppState` and a live runtime, and what it does
//! with them is write files and cancel tasks. This is the decision underneath, and it is the one
//! whose failure is silent: a detached child that is not marked forwards its launch to a parent
//! already exiting and goes down behind it, so a restart the user asked for leaves no window.

use std::ffi::OsStr;
use std::path::Path;

use super::{RESPAWN_ENV, detached_command};

/// What `single_instance::claim` looks for before it waits for the name instead of forwarding.
#[test]
fn a_detached_restart_marks_its_child() {
    let cmd = detached_command(Path::new("/usr/bin/Melodia"));

    let marked = cmd
        .get_envs()
        .find(|(key, _)| *key == OsStr::new(RESPAWN_ENV))
        .and_then(|(_, value)| value);

    assert_eq!(
        marked,
        Some(OsStr::new("1")),
        "an unmarked child forwards to a parent that is about to exit and dies with it",
    );
}
