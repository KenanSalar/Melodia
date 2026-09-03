//! Source pins for the ordered `views.json` index writes.
//!
//! The race fires only when one tick asks for two indices, and its symptom — a restart
//! opening on a page the user passed through — clears on the next navigation, so nothing
//! at runtime would ever report it. Releasing the writer lock early is the compiler's
//! (`clippy::let_underscore_lock`, denied through `correctness`); these cover what it
//! can't see.
//!
//! Six writers take `IndexPersist` — the sidebar's nav index and the five pages' tab
//! indices — and the lock itself is private to the primitive, so a call site can no longer
//! hold it wrongly. What is left to a test is the primitive's own critical section and the
//! two halves a caller still owns: writing inside the closure, and publishing before it
//! spawns.

use crate::test_support::{MIN_UI_SOURCES, UI_SRC_DIR, stripped_sources};

const PRIMITIVE: &str = include_str!("../index_persist.rs");

/// The three statements the primitive's critical section is made of.
const GUARD: &str = "let _writer = self.writer.lock();";
const LOAD: &str = "self.latest.load(";
const WRITE_CALL: &str = "write();";

/// How a *caller* reaches the primitive. The leading dot is what keeps the walks below off
/// the definition in `index_persist.rs`, which is a writer of nothing.
const CALL: &str = ".write_if_current(";

/// Brace-depth walk over `src[from..to]`, quote-aware, returning the **lowest** depth the
/// range reaches, or `None` where it closes a scope it never opened.
///
/// The lowest rather than the last, because [`inside_block`] is asking whether a scope was
/// ever left: a write hoisted out of the ordering closure and into any block after it ends
/// the range back above zero and reads as though it never moved.
///
/// `ui::scrollbar_tests`' walk asks the other way round: that one lifts a block's body,
/// this one asks whether two offsets share one. Comments are stripped by the caller for
/// the same reason the quotes are handled — a brace inside either unbalances the count.
/// Continuation bytes are all `>= 0x80`, so walking bytes can't mistake one for a brace.
fn depth_between(src: &str, from: usize, to: usize) -> Option<usize> {
    let between = src.get(from..to)?;
    let bytes = between.as_bytes();
    let mut depth = 0usize;
    let mut floor = usize::MAX;
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth = depth.checked_sub(1)?;
                floor = floor.min(depth);
            }
            _ => {}
        }
        i += 1;
    }
    Some(floor.min(depth))
}

/// Whether the guard bound at `guard` is still held once `needle` is reached — no
/// unmatched `}` closing its scope in between, and no hand-rolled `drop`.
fn guard_still_held(src: &str, guard: usize, needle: usize) -> bool {
    // `let_underscore_lock` catches the guard that is never bound; this catches the one
    // that is bound and then handed back before the write it was taken for.
    if src.get(guard..needle).is_some_and(|between| between.contains("drop(_writer")) {
        return false;
    }
    depth_between(src, guard, needle).is_some()
}

/// Whether `needle` is still inside the block whose `{` sits at `open`.
fn inside_block(src: &str, open: usize, needle: usize) -> bool {
    depth_between(src, open, needle).is_some_and(|depth| depth >= 1)
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

/// **The staleness load and the write share the writer's critical section.**
///
/// `nav_history::replay` fires the nav callback twice in a tick whenever it closes a
/// detail carrying a cross-section origin, and a page's tab takes a value per pick off a
/// bar the user can bounce; two `spawn_blocking` tasks have no ordering between them
/// whichever tick queued them, so the value the user only passed through can land last.
/// Four mutations are left to a test — dropping the load,
/// hoisting the write above it, taking the lock after it, and scoping the guard away from
/// the write. The third is the one that reads like an optimisation; the fourth is the one
/// `clippy::let_underscore_lock` looks like it covers and doesn't, since a guard that is
/// bound and then dropped is bound all the same.
///
/// The memory ordering is deliberately not pinned — `spawn_blocking` already gives each
/// task the edge to its own store, so `Relaxed` would be sound too.
#[test]
fn the_write_runs_under_the_lock_that_ordered_it() {
    let src = crate::test_support::strip_line_comments(PRIMITIVE);
    let body = src
        .split_once("fn write_if_current")
        .and_then(|(_, rest)| rest.split_once("\n    }\n"))
        .map_or("", |(body, _)| body);
    assert!(
        !body.is_empty(),
        "`write_if_current` moved or changed shape, so this pin reads nothing",
    );

    let guard = body.find(GUARD);
    let load = body.find(LOAD);
    let call = body.find(WRITE_CALL);
    assert!(
        guard.is_some(),
        "the write must hold the writer lock; it is the only ordering two spawned tasks get",
    );
    assert!(
        load.is_some(),
        "the write must skip an index the UI thread has moved past, or both writes run and \
         the loser decides what a restart opens on",
    );
    assert!(call.is_some(), "the write itself must still be reached");

    // Both are `Some` by the assertions above; `unwrap_or_default` is what keeps this off
    // `unwrap`, denied crate-wide.
    let (guard, load, call) =
        (guard.unwrap_or_default(), load.unwrap_or_default(), call.unwrap_or_default());
    assert!(
        guard < load,
        "the lock must be taken before the load — checking first reads like a cheap bail and \
         lets both tasks past, leaving them to race for the writer",
    );
    assert!(
        load < call,
        "the write must sit after the load, or the skip it performs decides nothing"
    );
    assert!(
        guard_still_held(body, guard, call),
        "the write must run *inside* the guard's scope — scoped into a block of its own or \
         dropped by hand, the lock is still in the diff and both tasks still clear the load, \
         so they go back to racing for who lands last",
    );
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
