//! Source pins for `Nav.persist-selected-index`'s disk write.
//!
//! Nothing at runtime catches a violation here: every spelling compiles, all of
//! them persist the right index on every path anyone tests by hand, and the wrong
//! ones lose a race that fires only when a single tick asks for two sections. What
//! it looks like is a restart opening on a page the user merely passed through —
//! self-correcting the moment they navigate, and so never reported as anything.
//!
//! The one mutation that *is* the compiler's is releasing the writer lock early
//! (`let _ = persist.writer.lock();`), which trips `clippy::let_underscore_lock` —
//! `correctness`, denied workspace-wide. What's left for these pins is the pieces
//! it can't see: that the staleness check exists, that the write sits after it, and
//! that the UI thread publishes ahead of the spawn.

const CALLBACKS: &str = include_str!("../mod.rs");

/// `mod.rs` with its comment lines dropped. These pins slice on the *absence* of a
/// call in one half of the closure, and the prose either side of that call names it
/// — so an unstripped search reports a lock-placement failure when someone edits a
/// sentence. The `my_library_tests` helper, applied to Rust.
fn code() -> String {
    CALLBACKS
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The handler body up to the spawn, and the disk closure from the spawn to its
/// `});` — the two halves every assertion below is about.
fn handler_and_disk_write(src: &str) -> (String, String) {
    let Some((_, rest)) = src.split_once("nav.on_persist_selected_index(") else {
        return (String::new(), String::new());
    };
    let Some((handler, body)) = rest.split_once("s.runtime.spawn_blocking(move || {") else {
        return (String::new(), String::new());
    };
    let disk = body.split_once("\n            });").map_or("", |(body, _)| body);
    (handler.to_owned(), disk.to_owned())
}

/// **The staleness load and the write share the writer's critical section.**
///
/// `nav_history::replay` fires this callback twice in a tick whenever it closes a
/// detail that a cross-section drill left an `origin-nav-index` on: once for the
/// section that detail's `close-detail` restores, once for the section the walk
/// actually names. Each spawns its own write, and two `spawn_blocking` tasks have
/// no ordering between them, so the origin can land last.
///
/// The two mutations left to a test — the compiler owns the third — are dropping
/// the check, and hoisting the write above it. Neither looks like anything.
#[test]
fn the_nav_index_write_is_ordered_against_the_tick_that_supersedes_it() {
    let src = code();
    let (handler, body) = handler_and_disk_write(&src);
    assert!(
        !body.is_empty(),
        "`Nav.persist-selected-index` no longer spawns its write the way this pin slices \
         for — if the persist moved, move the pin with it",
    );

    assert!(
        body.contains("let _write = persist.writer.lock();"),
        "the disk closure must hold the writer lock for the length of the write — it is \
         what orders two tasks the runtime hands no ordering to",
    );
    assert!(
        body.contains("if persist.latest.load(Ordering::Acquire) != idx {"),
        "the disk closure must skip an index the UI thread has already moved past; without \
         it both writes run and the loser decides what a restart opens on",
    );

    let (check, write) = body
        .split_once("if persist.latest.load(Ordering::Acquire) != idx {")
        .unwrap_or_default();
    assert!(
        !check.contains("set_last_nav_index(") && write.contains("set_last_nav_index("),
        "the write must sit *after* the staleness load and inside the guard's scope — that \
         is the whole ordering guarantee, not the load on its own",
    );

    // The other half, and the reason the lock can stay off the UI thread: the index
    // is published before the spawn. Moved inside the closure it would be written by
    // the racing tasks in whatever order they run, which is the ordering this exists
    // to stop depending on.
    assert!(
        handler.contains("persist.latest.store(idx, Ordering::Release);"),
        "the index must be published synchronously on the UI thread ahead of the spawn, or \
         a queued write has nothing newer to notice",
    );
    assert!(
        !handler.contains("writer.lock()"),
        "the UI thread must not take the writer lock — it is held across a `views.json` \
         round trip, and this line is on the path every sidebar click takes",
    );
}
