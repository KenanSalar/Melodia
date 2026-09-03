//! Every `views.json` index is written through the primitive that orders the writes.
//!
//! Two blocking tasks have no ordering between them whichever tick queued them, so a value the
//! user only passed through can land last. `IndexPersist` owns that argument; these ask whether
//! anything reaches around it, of the whole wiring tree rather than the six known sites.

use melodia_testkit::{MIN_UI_SOURCES, UI_SRC_DIR, stripped_sources};

/// How a *caller* reaches the primitive. The leading dot is what keeps the walks below off
/// the definition in `index_persist.rs`, which is a writer of nothing.
const CALL: &str = ".write_if_current(";

/// Whether `needle` is still inside the block whose `{` sits at `open`.
fn inside_block(src: &str, open: usize, needle: usize) -> bool {
    melodia_testkit::depth_between(src, open, needle).is_some_and(|depth| depth >= 1)
}

/// The wiring under [`UI_SRC_DIR`], less the pins over it — this file names both needles it
/// searches for, so a walk that read itself would answer about its own prose.
fn wiring_sources() -> Vec<(String, String)> {
    stripped_sources(UI_SRC_DIR, "rs", MIN_UI_SOURCES)
        .into_iter()
        .filter(|(rel, _)| !rel.split('/').any(|segment| segment == "tests"))
        .collect()
}

/// Every `views.json` index write in a source, **found rather than listed**: a sixth page
/// is a setter of its own, and a list is exactly what would not notice one.
///
/// The shape is `library::settings::set_<name>(`, kept when `<name>` is a tab index or the
/// nav index. Every other `library::settings` setter writes a value no second call in the
/// same tick competes for.
fn index_write_sites(src: &str) -> Vec<usize> {
    const PREFIX: &str = "library::settings::set_";
    src.match_indices(PREFIX)
        .filter_map(|(at, _)| {
            let name = src.get(at + PREFIX.len()..)?.split_once('(')?.0;
            let ordered = name.ends_with("_tab") || name == "last_nav_index";
            ordered.then_some(at)
        })
        .collect()
}

/// **Every persisted index is written from inside the ordering closure**, which is what
/// makes the primitive the only way to reach one of these setters.
///
/// The lock is private to `index_persist.rs`, so the failure this replaces — a call site
/// holding it wrongly — is now unspellable. What a call site can still do is write outside
/// the closure it was handed, which builds, persists, and is unordered exactly as before.
#[test]
fn every_persisted_index_is_written_inside_the_ordering_closure() {
    let mut sites = 0usize;
    for (rel, src) in wiring_sources() {
        let closures: Vec<usize> = src
            .match_indices(CALL)
            .filter_map(|(at, _)| src.get(at..).and_then(|rest| rest.find('{')).map(|rel| at + rel))
            .collect();

        for write in index_write_sites(&src) {
            sites += 1;
            assert!(
                closures.iter().any(|&open| inside_block(&src, open, write)),
                "{rel} writes a `views.json` index outside an `IndexPersist` closure — a \
                 bounce queues one value per tick and two blocking tasks have no ordering, \
                 so the index the user passed through can land last",
            );
        }
    }
    assert!(sites >= 6, "only {sites} index writes found — the walk is broken");
}

/// **The publish happens on the UI thread, ahead of the spawn.**
///
/// The half no privacy buys: a queued write reloads the shadow to decide whether it has
/// been superseded, so a value published *after* its own task was spawned leaves that task
/// comparing against the previous one — dropping the write that should have landed, or
/// landing the one that should have been dropped.
#[test]
fn every_writer_publishes_before_it_spawns() {
    let mut writers = 0usize;
    for (rel, src) in wiring_sources() {
        // Between the sites rather than ahead of each: searching the whole head would let a
        // second writer added below the first pass on the strength of the first one's publish.
        let mut prev = 0usize;
        for (write, _) in src.match_indices(CALL) {
            writers += 1;
            assert!(
                src.get(prev..write).is_some_and(|between| between.contains(".publish(")),
                "{rel} must publish its index before the task that writes it — a queued write \
                 has nothing newer to notice otherwise",
            );
            prev = write;
        }
    }
    assert_eq!(
        writers, 6,
        "six writers take `IndexPersist` — the nav index and the five pages' tabs. A new one \
         is welcome; a *missing* one is a page that went back to writing unordered",
    );
}
