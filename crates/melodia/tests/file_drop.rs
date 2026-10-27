//! What a file drop's targets owe the hit test the winit filter runs for them.
//!
//! Slint has no pointer during an OS drag, so each target publishes where it is drawn and Rust
//! decides where letting go lands (`desktop-shell.md`). Both halves fail away from the code that
//! breaks them: a report in the wrong branch panics on a frame nothing obvious caused, and a second
//! writer of the answer raises a banner over a target the files won't reach.

use std::collections::BTreeMap;

use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, mount_routes, spellings_outside, stripped_sources,
};

/// The one branch a report may sit in, which the miniplayer swap drops on a tick of its own.
const FULL_UI: &str = "if !mini-switch.render-active: Rectangle";

const REPORT: &str = "WindowRectReport";

fn sources() -> Vec<(String, String)> {
    stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
}

/// **A `WindowRectReport` sits inside no `if` or `for` but the full-UI branch.** Its tracker
/// watches its own geometry, so a branch dropped while its parent re-flows panics on it. Climbed
/// rather than read per file: the queue panel's report sits in no branch of its own file, and the
/// panel's mount is what decides.
#[test]
fn every_window_rect_report_sits_in_no_branch_but_the_full_ui() {
    let tree = sources();
    let mut offenders = Vec::new();

    for (path, routes) in mount_routes(&tree, REPORT) {
        for route in routes {
            let branches: Vec<&str> = route
                .iter()
                .map(String::as_str)
                .filter(|head| head.starts_with("if ") || head.starts_with("for "))
                .collect();
            if branches != [FULL_UI] {
                offenders.push(format!("{path}: under {branches:?}"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a report is mounted outside every `if` and `for` but `{FULL_UI}`:\n{}",
        offenders.join("\n")
    );
}

/// **Two reports, one per rect `FileDrag` holds.** A second writer of either rect moves the hit
/// test off the target it names, and a third target owes `window_chrome::file_drop` an arm of its
/// own beside its report.
#[test]
fn each_file_drop_rect_has_one_report() {
    let tree = sources();
    let mut reports: BTreeMap<&str, usize> = BTreeMap::new();

    for (path, _) in mount_routes(&tree, REPORT) {
        *reports.entry(path).or_insert(0) += 1;
    }

    assert_eq!(
        reports,
        BTreeMap::from([("app-window.slint", 1), ("views/queue-panel/queue-panel.slint", 1)]),
        "the content panel and the queue panel's body each report once"
    );
}

/// **The winit filter is the one Rust writer of `FileDrag.target`.** Both banners read it, which
/// is what keeps them naming where letting go lands, the filter writing the answer it routes the
/// drop by. The count is the filter's three writes: a move, a leave and a drop.
#[test]
fn no_rust_but_the_winit_filter_writes_the_file_drop_target() {
    assert_eq!(
        spellings_outside("set_target(", &[("ui/window_chrome/winit_filter.rs", 3)]),
        Vec::<String>::new(),
        "only the winit filter's hit test may write `FileDrag.target`"
    );
}

/// **No `.slint` file writes `FileDrag.target`.** A banner raised from Slint would answer a
/// question only the hit test can, there being no pointer during the drag to ask.
#[test]
fn no_slint_file_writes_the_file_drop_target() {
    const TARGET: &str = "FileDrag.target";
    let tree = sources();
    let mut reads = 0usize;
    let mut writers = Vec::new();

    for (path, src) in &tree {
        for (at, _) in src.match_indices(TARGET) {
            let after = src[at + TARGET.len()..].trim_start();
            let assigned = after.starts_with('=') && !after.starts_with("==");
            let linked = after.starts_with("<=>") || src[..at].trim_end().ends_with("<=>");
            if assigned || linked {
                writers.push(path.as_str());
            } else {
                reads += 1;
            }
        }
    }

    assert!(reads >= 2, "only {reads} reads of `{TARGET}` found: the walk is broken");
    assert!(writers.is_empty(), "`{TARGET}` is the winit filter's to write: {writers:?}");
}
