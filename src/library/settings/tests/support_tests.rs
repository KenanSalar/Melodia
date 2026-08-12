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
    assert_eq!(
        flags.launch_count, PROMPT_AT_LAUNCH,
        "a seen install kept counting"
    );
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
