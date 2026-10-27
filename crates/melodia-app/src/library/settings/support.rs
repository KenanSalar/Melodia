//! What the one-time Ko-fi prompt persists, the decision split from the disk write so it is
//! testable without a settings file.

use crate::services;
use crate::services::settings::SupportFlags;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Which launch asks. Not the first: someone still deciding whether to keep the app
/// has nothing to decide about supporting it yet.
pub const PROMPT_AT_LAUNCH: u32 = 5;

/// Count this launch and answer whether it is the one that asks. Counting stops once the prompt
/// has been shown, so a settled install stops rewriting `settings.json` at boot.
pub fn count_launch(flags: &mut SupportFlags) -> bool {
    if flags.support_prompt_seen {
        return false;
    }
    flags.launch_count = flags.launch_count.saturating_add(1);
    flags.launch_count >= PROMPT_AT_LAUNCH
}

/// Record this launch, and say whether the prompt is due.
///
/// The read ahead of the mutate is what keeps [`count_launch`]'s claim true: `mutate_settings_with`
/// writes unconditionally, so without it a settled install would rewrite `settings.json` every
/// boot. It needn't sit under `MUTATE_LOCK`: `count_launch` re-checks the copy
/// `mutate_settings_with` re-reads, so a stale `false` costs a redundant write and never re-raises
/// a spent prompt.
pub fn record_launch(paths: &Paths) -> Result<bool, AppError> {
    if services::settings::read_settings(paths)?.support.support_prompt_seen {
        return Ok(false);
    }

    services::settings::mutate_settings_with(paths, |settings| count_launch(&mut settings.support))
}

/// Spend the one prompt.
///
/// Called immediately before the toast is raised, never on click or dismiss: a
/// dismissed toast must not return next launch, and a session that ends before the
/// toast is due must not have spent it.
pub fn mark_support_prompt_seen(paths: &Paths) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, |settings| {
        settings.support.support_prompt_seen = true;
    })
}

#[cfg(test)]
#[path = "tests/support_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/support_writers_tests.rs"]
mod writer_tests;
