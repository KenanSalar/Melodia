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

const PRIMITIVE: &str = include_str!("../index_persist.rs");

/// The three statements the primitive's critical section is made of.
const GUARD: &str = "let _writer = self.writer.lock();";
const LOAD: &str = "self.latest.load(";
const WRITE_CALL: &str = "write();";

/// Whether the guard bound at `guard` is still held once `needle` is reached — no
/// unmatched `}` closing its scope in between, and no hand-rolled `drop`.
fn guard_still_held(src: &str, guard: usize, needle: usize) -> bool {
    // `let_underscore_lock` catches the guard that is never bound; this catches the one
    // that is bound and then handed back before the write it was taken for.
    if src.get(guard..needle).is_some_and(|between| between.contains("drop(_writer")) {
        return false;
    }
    melodia_testkit::depth_between(src, guard, needle).is_some()
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
    let src = melodia_testkit::strip_line_comments(PRIMITIVE);
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
