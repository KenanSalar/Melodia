//! Replacing the user's home directory with `~` in anything they are asked to attach to a
//! public issue.

use std::borrow::Cow;

/// Replace the user's home directory with `~` throughout `text`.
///
/// Everything a crash report or diagnostics bundle carries goes through this before reaching a
/// file the user is asked to attach to a public issue — a home directory usually holds a real name.
///
/// The home directory comes from [`dirs::home_dir`], **not** `$HOME`; the root `CLAUDE.md` argues
/// why, and the short of it is that the variable is normally unset on Windows, exactly where this
/// earns its keep.
///
/// Resolved per call rather than cached, which is a trade: four tests across three files drive
/// this through `$HOME`, so a process-wide cache would put the answer out of their reach. A bundle
/// makes tens of these and a crash report two; anything hotter wants the answer passed in rather
/// than a cache the tests can't reset.
pub fn redact_home(text: &str) -> Cow<'_, str> {
    let Some(home) = home_dir_string() else {
        return Cow::Borrowed(text);
    };
    redact_prefix(text, &home)
}

/// The home directory as a string, or `None` when there isn't one to redact.
///
/// Reachable from `test_support::resolved_home` so a redaction fixture is built from the same
/// answer the redaction reads, rather than from a second guess at it.
pub(crate) fn home_dir_string() -> Option<String> {
    let home = dirs::home_dir()?;
    let home = home.to_str()?;
    (!home.is_empty()).then(|| home.to_owned())
}

/// The pure half of [`redact_home`]. Borrows when there is nothing to replace,
/// which is the common case.
fn redact_prefix<'a>(text: &'a str, home: &str) -> Cow<'a, str> {
    if !text.contains(home) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace(home, "~"))
}

#[cfg(test)]
#[path = "tests/redact_tests.rs"]
mod tests;
