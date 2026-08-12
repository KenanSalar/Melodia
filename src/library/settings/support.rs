//! What the one-time Ko-fi prompt persists.
//!
//! The decision is split from the disk write so the sequence can be tested without a
//! settings file — the [`crate::tasks::audio_health`] shape.

use crate::error::AppError;
use crate::services;
use crate::services::settings::SupportFlags;
use crate::state::AppState;

/// Which launch asks. Not the first: someone still deciding whether to keep the app
/// has nothing to decide about supporting it yet.
pub const PROMPT_AT_LAUNCH: u32 = 5;

/// Count this launch and answer whether it is the one that asks.
///
/// Saturating rather than wrapping, though neither is reachable — counting stops the
/// moment the prompt has been shown, so a settled install stops rewriting
/// `settings.json` at boot instead of climbing forever.
pub fn count_launch(flags: &mut SupportFlags) -> bool {
    if flags.support_prompt_seen {
        return false;
    }
    flags.launch_count = flags.launch_count.saturating_add(1);
    flags.launch_count >= PROMPT_AT_LAUNCH
}

/// Record this launch, and say whether the prompt is due.
///
/// The read ahead of the mutate is what keeps the claim above true:
/// `mutate_settings_with` writes unconditionally, so without it a settled install
/// would rewrite `settings.json` on every boot to store a counter it had stopped
/// advancing. It sits outside `MUTATE_LOCK` and doesn't need to be inside it —
/// `count_launch` re-checks the flag against the copy `mutate_settings_with` re-reads
/// from disk, so a stale `false` costs a redundant write and can never re-raise a spent
/// prompt. That re-read is what carries it rather than the lock:
/// `mark_support_prompt_seen` lands `PROMPT_DELAY` later in this process, so the only
/// writer that can race the read below is a second one, which a process-local `static`
/// doesn't reach either way.
pub fn record_launch(state: &AppState) -> Result<bool, AppError> {
    if services::settings::read_settings(&state.paths)?
        .support
        .support_prompt_seen
    {
        return Ok(false);
    }

    services::settings::mutate_settings_with(&state.paths, |settings| {
        count_launch(&mut settings.support)
    })
}

/// Spend the one prompt.
///
/// Called immediately before the toast is raised, never on click or dismiss: a
/// dismissed toast must not return next launch, and a session that ends before the
/// toast is due must not have spent it.
pub fn mark_support_prompt_seen(state: &AppState) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, |settings| {
        settings.support.support_prompt_seen = true;
    })
}

#[cfg(test)]
#[path = "tests/support_tests.rs"]
mod tests;
