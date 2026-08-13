use super::{PROMPT_AT_LAUNCH, count_launch};
use crate::services::settings::SupportFlags;

/// Walks a fresh install up to the prompt and past it.
///
/// The interesting assertion is the last one: once the prompt has been spent the
/// counter stops moving, which is what keeps a settled install from rewriting
/// `settings.json` on every boot.
#[test]
fn the_prompt_is_due_once_and_the_counter_then_stops() {
    let mut flags = SupportFlags::default();

    for launch in 1..PROMPT_AT_LAUNCH {
        assert!(
            !count_launch(&mut flags),
            "launch {launch} asked, but the prompt is due at {PROMPT_AT_LAUNCH}"
        );
        assert_eq!(flags.launch_count, launch);
    }

    assert!(count_launch(&mut flags), "launch {PROMPT_AT_LAUNCH} should ask");
    assert_eq!(flags.launch_count, PROMPT_AT_LAUNCH);

    // The caller flips this once the toast is actually raised.
    flags.support_prompt_seen = true;
    assert!(!count_launch(&mut flags));
    assert_eq!(flags.launch_count, PROMPT_AT_LAUNCH, "a seen install kept counting");
}

/// A session that ends before the toast is due spends the increment and not the flag,
/// so the launch after it asks instead of the chance being lost.
#[test]
fn a_launch_past_the_threshold_still_asks_while_unseen() {
    let mut flags = SupportFlags {
        launch_count: PROMPT_AT_LAUNCH,
        support_prompt_seen: false,
    };

    assert!(count_launch(&mut flags));
    assert_eq!(flags.launch_count, PROMPT_AT_LAUNCH + 1);
}

/// `count_launch` decides not to count; only `record_launch` can decide not to
/// *write*, and `mutate_settings_with` writes whatever the closure leaves behind.
/// So the guard above it is the whole of what makes three separate doc comments
/// true — this module's two and `SupportFlags`' — and deleting it restores a
/// per-boot rewrite of `settings.json` with both tests above still green.
///
/// Comments are stripped first, and that guards the *inside* of the window rather
/// than its edges — `record_launch`'s own prose sits above the split and is never
/// searched, but `body` runs on to the next `pub fn`, so it carries any in-body
/// comment plus `mark_support_prompt_seen`'s doc. A guard deleted and replaced by a
/// comment saying what it used to do satisfies every needle below verbatim.
#[test]
fn a_spent_prompt_is_read_before_the_settings_file_is_rewritten() {
    let src = crate::test_support::strip_line_comments(include_str!("../support.rs"));
    let tail = src.split_once("pub fn record_launch").map(|(_, tail)| tail).unwrap_or_default();
    let body = tail.split_once("\npub fn").map_or(tail, |(body, _)| body);
    assert!(!body.is_empty(), "`record_launch` is gone from support.rs");

    let read = body.find("read_settings");
    let mutate = body.find("mutate_settings_with");
    assert!(read.is_some(), "`record_launch` no longer reads the flag");
    assert!(mutate.is_some(), "`record_launch` no longer counts through `mutate_settings_with`");
    let (Some(read), Some(mutate)) = (read, mutate) else {
        return;
    };

    assert!(
        read < mutate,
        "the read has to guard the mutate, or a settled install rewrites settings.json at boot"
    );

    let guard = body.get(..mutate).unwrap_or_default();
    assert!(
        guard.contains("support_prompt_seen") && guard.contains("return Ok(false)"),
        "the read has to bail on a spent prompt, not merely happen first"
    );
}
