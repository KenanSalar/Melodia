//! Source pins for `Nav.persist-selected-index`'s disk write.
//!
//! The race fires only when one tick asks for two sections, and its symptom — a
//! restart opening on a page the user passed through — clears on the next
//! navigation, so nothing at runtime would ever report it. Releasing the writer
//! lock early is the compiler's (`clippy::let_underscore_lock`, denied through
//! `correctness`); these cover what it can't see.

const CALLBACKS: &str = include_str!("../mod.rs");

/// `mod.rs` less its comment lines — the assertions below turn on a call being
/// *absent* from one half of the closure, and the prose either side names it.
fn code() -> String {
    CALLBACKS
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The handler up to the spawn, and the disk closure from the spawn to its `});`.
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
/// `replay` fires this callback twice in a tick whenever it closes a detail carrying
/// a cross-section origin, and two `spawn_blocking` tasks have no ordering between
/// them, so the origin can land last. The mutations left to a test are dropping the
/// load and hoisting the write above it; neither looks like anything.
///
/// The memory ordering is deliberately not pinned — `spawn_blocking` already gives
/// each task the edge to its own store, so `Relaxed` would be sound too.
#[test]
fn the_nav_index_write_is_ordered_against_the_tick_that_supersedes_it() {
    let src = code();
    let (handler, body) = handler_and_disk_write(&src);
    assert!(
        !body.is_empty(),
        "the persist no longer spawns its write the way this pin slices for — move it along",
    );

    assert!(
        body.contains("let _write = persist.writer.lock();"),
        "the disk closure must hold the writer lock across the write; it is the only \
         ordering two spawned tasks get",
    );

    let (check, write) = body
        .split_once("persist.latest.load(")
        .and_then(|(head, tail)| tail.split_once(") != idx {").map(|(_, w)| (head, w)))
        .unwrap_or_default();
    assert!(
        !write.is_empty(),
        "the disk closure must skip an index the UI thread has moved past, or both writes \
         run and the loser decides what a restart opens on",
    );
    assert!(
        !check.contains("set_last_nav_index(") && write.contains("set_last_nav_index("),
        "the write must sit after the load, inside the guard's scope",
    );

    assert!(
        handler.contains("persist.latest.store(idx,"),
        "the index must be published on the UI thread ahead of the spawn, or a queued write \
         has nothing newer to notice",
    );
    assert!(
        !handler.contains("writer.lock()"),
        "the UI thread must not take the writer lock — it is held across a `views.json` \
         round trip, on the path every sidebar click takes",
    );
}
