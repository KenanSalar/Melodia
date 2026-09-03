//! Nothing outside `utils::exe` asks the OS for the binary path.
//!
//! `std::env::current_exe` is a bare `readlink("/proc/self/exe")` on Linux, and the kernel
//! appends a literal `" (deleted)"` once the dentry is unlinked — which an RPM or DEB upgrade
//! does to `/usr/bin/Melodia` mid-session, and cargo does to `target/debug/Melodia` on every
//! re-uplift. The helper resolves that rather than trimming it.

use melodia_testkit::spellings_outside;

/// The raw call, in the one spelling that can't be dodged: every path form has to name the module
/// (`std::env::current_exe`, or `env::current_exe` under a `use std::env`). The evasion left is a
/// `use std::env::current_exe` and a bare call, the same hole `ui::file_dialog`'s pin documents
/// for a renaming `use`.
const RAW_CALL: &str = "env::current_exe";

/// The files that may spell it, and **how many times each**. A count rather than a file-level
/// pass: `utils/exe.rs` holds both the helper and the one sanctioned raw reader, so forgiving
/// the file would pre-authorise a third call written between them. Paths are relative to the
/// crate root that holds them.
/// One entry rather than two: this pin used to name itself, having to spell the needle it greps
/// for, and out of the corpus it no longer does.
const EXEMPT: [(&str, usize); 1] = [
    // `current_exe` itself, plus `is_dev_build` — which takes `parent()/parent()`, so the marker
    // lands on a file name it never looks at.
    ("utils/exe.rs", 2),
];

/// A test comparing `install_target()` against `utils::exe::current_exe()` cannot fail: with no
/// marker in the test process the two agree, so it passes just as well against the raw call it
/// exists to rule out. The routing is only checkable from the corpus, and what this guards is a
/// *next* call site rather than any existing one.
///
/// Its reach is every crate's sources, where a call that matters can live — a binary path is
/// executed, installed to or written down by the app, not by `tests/` or a build script.
#[test]
fn nothing_outside_the_helper_asks_the_os_for_the_binary_path() {
    let raw = spellings_outside(RAW_CALL, &EXEMPT);

    assert!(
        raw.is_empty(),
        "{raw:?} ask the OS for the binary path directly — use `utils::exe::current_exe`, \
         which resolves the `\" (deleted)\"` marker an unlinked executable gets"
    );
}
