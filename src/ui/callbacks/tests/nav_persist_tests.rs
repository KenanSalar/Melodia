//! Source pins for `Nav.persist-selected-index`'s disk write.
//!
//! Nothing at runtime catches a violation here: both spellings compile, both
//! persist the right index on every path anyone tests by hand, and the one that
//! is wrong loses a race that fires only when a single tick asks for two
//! sections. What it looks like is a restart opening on a page the user merely
//! passed through — self-correcting the moment they navigate, and so never
//! reported as anything.

const CALLBACKS: &str = include_str!("../mod.rs");

/// The disk closure, from the `spawn_blocking` that owns it to its `});`.
fn disk_write() -> String {
    CALLBACKS
        .split_once("nav.on_persist_selected_index(")
        .and_then(|(_, rest)| rest.split_once("s.runtime.spawn_blocking(move || {"))
        .and_then(|(_, body)| body.split_once("\n            });"))
        .map_or(String::new(), |(body, _)| body.to_owned())
}

/// **The staleness check and the write share one critical section.**
///
/// `nav_history::replay` fires this callback twice in a tick whenever it closes a
/// detail that a cross-section drill left an `origin-nav-index` on: once for the
/// section that detail's `close-detail` restores, once for the section the walk
/// actually names. Each spawns its own write, and two `spawn_blocking` tasks have
/// no ordering between them, so the origin can land last.
///
/// Two mutations put that back and neither looks like anything: dropping the
/// check, and — the one worth the pin — hoisting the read out of the guard's
/// lifetime (`let current = *shadow.lock();`), which reads identically and
/// releases the lock before the write, leaving the two tasks unordered again.
#[test]
fn the_nav_index_write_is_ordered_against_the_tick_that_supersedes_it() {
    let body = disk_write();
    assert!(
        !body.is_empty(),
        "`Nav.persist-selected-index` no longer spawns its write the way this pin slices \
         for — if the persist moved, move the pin with it",
    );

    assert!(
        body.contains("let current = shadow.lock();"),
        "the disk closure must bind the *guard*, not a copy — `let current = *shadow.lock();` \
         drops the lock on the same line, so a superseded write can still be scheduled after \
         the newer one",
    );
    assert!(
        body.contains("if *current != idx {"),
        "the disk closure must skip an index the shadow has already moved past; without it \
         both writes run and the loser decides what a restart opens on",
    );

    let (check, write) = body.split_once("if *current != idx {").unwrap_or_default();
    assert!(
        !check.contains("set_last_nav_index(") && write.contains("set_last_nav_index("),
        "the write must sit *after* the staleness check and inside the guard's scope — that \
         is the whole ordering guarantee, not the check on its own",
    );

    // The other half: the shadow has to move on the UI thread, before the spawn.
    // Moved inside the closure it would be written by the racing tasks in whatever
    // order they run, which is the ordering this exists to stop depending on.
    let handler = CALLBACKS
        .split_once("nav.on_persist_selected_index(")
        .and_then(|(_, rest)| rest.split_once("s.runtime.spawn_blocking(move || {"))
        .map_or("", |(head, _)| head);
    assert!(
        handler.contains("*shadow.lock() = idx;"),
        "the shadow must be advanced synchronously on the UI thread ahead of the spawn, or a \
         queued write has nothing newer to notice",
    );
}
