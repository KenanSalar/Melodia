//! Source pins for `Nav.persist-selected-index`'s disk write.
//!
//! The race fires only when one tick asks for two sections, and its symptom — a
//! restart opening on a page the user passed through — clears on the next
//! navigation, so nothing at runtime would ever report it. Releasing the writer
//! lock early is the compiler's (`clippy::let_underscore_lock`, denied through
//! `correctness`); these cover what it can't see.

const CALLBACKS: &str = include_str!("../mod.rs");

/// The two statements every assertion here is about — the guard, and the call it has
/// to still be holding.
const GUARD: &str = "let _write = persist.writer.lock();";
const WRITE_CALL: &str = "library::settings::set_last_nav_index(";

/// `mod.rs` less its comment lines — the assertions below turn on a call being
/// *absent* from one half of the closure, and the prose either side names it.
fn code() -> String {
    CALLBACKS
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the guard bound at `guard` is still held once `needle` is reached — no
/// unmatched `}` closing its scope in between, and no hand-rolled `drop`.
///
/// `ui::scrollbar_tests`' brace walk asks the other way round: that one lifts a block's
/// own body, this one asks whether two statements share one. Quote-aware like its
/// sibling so a brace inside a string can't unbalance the count, and the caller strips
/// comments for the same reason — neither is exercised by the closure as it stands.
/// Continuation bytes are all `>= 0x80`, so walking bytes can't mistake one for a brace.
fn guard_still_held(src: &str, guard: usize, needle: usize) -> bool {
    let Some(between) = src.get(guard..needle) else {
        return false;
    };
    // `let_underscore_lock` catches the guard that is never bound; this catches the
    // one that is bound and then handed back before the write it was taken for.
    if between.contains("drop(_write") {
        return false;
    }
    let bytes = between.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                // Below the depth we started at is the guard's own scope closing.
                let Some(outer) = depth.checked_sub(1) else {
                    return false;
                };
                depth = outer;
            }
            _ => {}
        }
        i += 1;
    }
    true
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
/// them, so the origin can land last. Four mutations are left to a test — dropping the
/// load, hoisting the write above it, taking the lock after it, and scoping the guard
/// away from the write. The third is the one that reads like an optimisation; the
/// fourth is the one `clippy::let_underscore_lock` looks like it covers and doesn't,
/// since a guard that is bound and then dropped is bound all the same.
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
        body.contains(GUARD),
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
    // Path-qualified, which is how the closure spells it and how every `library::settings`
    // call in `src/ui/` does: the bare name also matches the `log::warn!` naming the
    // function it failed in, which sits *after* the call today and would quietly become
    // the anchor if anything logged ahead of it.
    assert!(
        !check.contains(WRITE_CALL) && write.contains(WRITE_CALL),
        "the write must sit after the load, or the skip it performs decides nothing",
    );
    assert!(
        check.contains(GUARD),
        "the lock must be taken before the load — checking first reads like a cheap bail and \
         lets both tasks past, leaving them to race for the writer",
    );

    // Both are `Some` by the assertions above; `unwrap_or_default` is what keeps this
    // off `unwrap`, denied crate-wide.
    let guard = body.find(GUARD).unwrap_or_default();
    let call = body.find(WRITE_CALL).unwrap_or_default();
    assert!(
        guard_still_held(&body, guard, call),
        "the write must run *inside* the guard's scope — scoped into a block of its own or \
         dropped by hand, the lock is still in the diff and both tasks still clear the load, \
         so they go back to racing for who lands last",
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
