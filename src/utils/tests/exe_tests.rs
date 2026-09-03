use std::path::{Path, PathBuf};

use super::undeleted_exe;
use crate::test_support::spellings_outside;

/// Every process asks for its own path at least once a run, and almost none of them is running
/// from an unlinked file — so the suffix test has to come before the filesystem. Counted rather
/// than asserted on the result, since returning the path unchanged is what *both* orderings do.
#[test]
fn a_path_without_the_marker_costs_no_filesystem_question() {
    let asked = std::cell::Cell::new(0_u32);
    let exe = PathBuf::from("/usr/bin/Melodia");

    let resolved = undeleted_exe(exe.clone(), |_| {
        asked.set(asked.get() + 1);
        true
    });

    assert_eq!(resolved, exe);
    assert_eq!(asked.get(), 0);
}

/// A file genuinely named `… (deleted)` is not this bug, and redirecting it to a sibling would be
/// a worse failure than the one being fixed — silently running a different binary. The live-file
/// guard is the only thing separating the two cases, both of which end in the marker.
#[test]
fn a_live_file_named_deleted_keeps_its_own_path() {
    let odd = PathBuf::from("/srv/builds/Melodia (deleted)");

    // Both it and its would-be sibling exist; the suffixed one wins.
    let resolved = undeleted_exe(odd.clone(), |_| true);

    assert_eq!(resolved, odd);
}

/// The case this exists for: a package upgrade (or a cargo re-uplift) unlinked the running binary
/// and put its replacement at the same path. Resolving to that replacement is what makes the
/// respawn relaunch what the user now has installed rather than dying.
#[test]
fn an_unlinked_binary_resolves_to_the_file_that_replaced_it() {
    let replacement = Path::new("/usr/bin/Melodia");

    let resolved = undeleted_exe(PathBuf::from("/usr/bin/Melodia (deleted)"), |p| p == replacement);

    assert_eq!(resolved, replacement);
}

/// Nothing at either path — the binary was uninstalled rather than replaced. Handing back the
/// kernel's own string keeps the caller's error report honest about what it was told.
#[test]
fn an_unresolvable_marker_comes_back_verbatim() {
    let reported = PathBuf::from("/usr/bin/Melodia (deleted)");

    let resolved = undeleted_exe(reported.clone(), |_| false);

    assert_eq!(resolved, reported);
}

/// The raw call, in the one spelling that can't be dodged: every path form has to name the module
/// (`std::env::current_exe`, or `env::current_exe` under a `use std::env`). The evasion left is a
/// `use std::env::current_exe` and a bare call, the same hole `ui::file_dialog`'s pin documents
/// for a renaming `use`.
const RAW_CALL: &str = "env::current_exe";

/// The files that may spell it, and **how many times each**. A count rather than a file-level
/// pass: `utils/exe.rs` holds both the helper and the one sanctioned raw reader, so forgiving
/// the file would pre-authorise a third call written between them. Paths are relative to
/// `SRC_DIR`.
const EXEMPT: [(&str, usize); 2] = [
    // `current_exe` itself, plus `is_dev_build` — which takes `parent()/parent()`, so the marker
    // lands on a file name it never looks at.
    ("utils/exe.rs", 2),
    // This pin, which has to spell the needle to grep for it.
    ("utils/tests/exe_tests.rs", 1),
];

/// A test comparing `install_target()` against `utils::exe::current_exe()` cannot fail: with no
/// marker in the test process the two agree, so it passes just as well against the raw call it
/// exists to rule out. The routing is only checkable from the corpus, and what this guards is a
/// *next* call site rather than any existing one.
///
/// Its reach is `SRC_DIR`, where a call that matters can live — a binary path is executed,
/// installed to or written down by the app, not by `tests/` or a build script.
#[test]
fn nothing_outside_the_helper_asks_the_os_for_the_binary_path() {
    let raw = spellings_outside(RAW_CALL, &EXEMPT);

    assert!(
        raw.is_empty(),
        "{raw:?} ask the OS for the binary path directly — use `utils::exe::current_exe`, \
         which resolves the `\" (deleted)\"` marker an unlinked executable gets"
    );
}
