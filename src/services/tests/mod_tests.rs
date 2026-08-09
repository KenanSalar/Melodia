use std::borrow::Cow;

#[cfg(unix)]
use super::redact_home;
use super::redact_prefix;
#[cfg(unix)]
use crate::test_support::with_env_var;

/// A Windows home directory, spelled the way `Path::display` spells it. The
/// resolution used to go through `$HOME`, which Windows does not set — so every
/// path in a crash report and in the bundle a user attaches to a public issue
/// shipped the account name verbatim. Nothing on a Linux runner can exercise
/// `dirs::home_dir`'s Windows arm, so the *shape* is pinned here instead.
#[test]
fn a_windows_home_is_redacted_like_a_unix_one() {
    let redacted = redact_prefix(
        r"WARN scan failed for C:\Users\Alice\Music\x.flac",
        r"C:\Users\Alice",
    );

    assert_eq!(redacted, r"WARN scan failed for ~\Music\x.flac");
}

/// Every occurrence, not just the first: one log line can name a source path and
/// a destination, and a backtrace names the home once per frame.
#[test]
fn every_occurrence_goes() {
    let redacted = redact_prefix(
        "moved /home/alice/a.flac to /home/alice/b.flac",
        "/home/alice",
    );

    assert_eq!(redacted, "moved ~/a.flac to ~/b.flac");
}

/// The common case is a line with no home in it at all, and it must not
/// allocate a copy of every log record on the way past.
#[test]
fn text_without_the_home_is_borrowed() {
    let borrowed = redact_prefix("INFO Melodia starting", "/home/alice");
    assert!(matches!(borrowed, Cow::Borrowed(_)));
}

/// The Unix half of the resolution, which is what the crash-report and
/// diagnostics tests lean on: `dirs::home_dir` reads `$HOME` before it falls
/// back to the password database. Gated, because that is exactly the arm
/// Windows doesn't have — `known_folder_profile` ignores the variable — and the
/// point of the whole change is that the two platforms resolve differently.
#[cfg(unix)]
#[test]
fn the_home_directory_comes_from_the_environment_on_unix() {
    let redacted = with_env_var("HOME", Some("/home/testuser"), || {
        redact_home("/home/testuser/Music/x.flac").into_owned()
    });

    assert_eq!(redacted, "~/Music/x.flac");
}
