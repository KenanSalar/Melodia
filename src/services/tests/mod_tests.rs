use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use super::redact_home;
use super::{redact_prefix, undeleted_exe};
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

/// Every process asks for its own path at least once a run, and almost none of
/// them is running from an unlinked file — so the suffix test has to come before
/// the filesystem, not after it. Counted rather than asserted on the result,
/// since returning the path unchanged is what *both* orderings do.
#[test]
fn a_path_without_the_marker_costs_no_filesystem_question() {
    let asked = std::cell::Cell::new(0_u32);
    let exe = PathBuf::from("/usr/bin/Melodia");

    let resolved = undeleted_exe(exe.clone(), |_| {
        asked.set(asked.get() + 1);
        true
    });

    assert_eq!(resolved, exe);
    assert_eq!(asked.get(), 0);
}

/// A file genuinely named `… (deleted)` is not this bug, and redirecting it to a
/// sibling would be a worse failure than the one being fixed — silently running
/// a different binary. The live-file guard is the only thing separating the two
/// cases, both of which end in the marker.
#[test]
fn a_live_file_named_deleted_keeps_its_own_path() {
    let odd = PathBuf::from("/srv/builds/Melodia (deleted)");

    // Both it and its would-be sibling exist; the suffixed one wins.
    let resolved = undeleted_exe(odd.clone(), |_| true);

    assert_eq!(resolved, odd);
}

/// The case this exists for: a package upgrade (or a cargo re-uplift) unlinked
/// the running binary and put its replacement at the same path. Resolving to
/// that replacement is what makes the respawn relaunch what the user now has
/// installed rather than dying.
#[test]
fn an_unlinked_binary_resolves_to_the_file_that_replaced_it() {
    let replacement = Path::new("/usr/bin/Melodia");

    let resolved = undeleted_exe(
        PathBuf::from("/usr/bin/Melodia (deleted)"),
        |p| p == replacement,
    );

    assert_eq!(resolved, replacement);
}

/// Nothing at either path — the binary was uninstalled rather than replaced.
/// Handing back the kernel's own string keeps the caller's error report honest
/// about what it was told.
#[test]
fn an_unresolvable_marker_comes_back_verbatim() {
    let reported = PathBuf::from("/usr/bin/Melodia (deleted)");

    let resolved = undeleted_exe(reported.clone(), |_| false);

    assert_eq!(resolved, reported);
}
