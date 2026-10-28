//! Replacing the user's home directory with `~` in anything they are asked to attach to a
//! public issue.

use std::borrow::Cow;

/// Through [`dirs::home_dir`], never `$HOME`, which Windows leaves unset: its users have no console
/// to have read the paths from, and `README.md` and `bug_report.yml` both promise them `~`.
/// Resolved per call rather than cached, so the tests driving it through `$HOME` can still reach
/// the answer.
pub fn redact_home(text: &str) -> Cow<'_, str> {
    let Some(home) = home_dir_string() else {
        return Cow::Borrowed(text);
    };
    redact_prefix(text, &home)
}

/// What a redaction test builds its fixture from: `dirs::home_dir()` never reads a faked `$HOME`
/// on Windows.
pub fn home_dir_string() -> Option<String> {
    let home = dirs::home_dir()?;
    let home = home.to_str()?;
    (!home.is_empty()).then(|| home.to_owned())
}

/// Borrows in the common case, with nothing to replace.
fn redact_prefix<'a>(text: &'a str, home: &str) -> Cow<'a, str> {
    if !text.contains(home) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace(home, "~"))
}

#[cfg(test)]
#[path = "tests/redact_tests.rs"]
mod tests;
