use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use super::redact_home;
use super::{describe, redact_prefix, undeleted_exe};
use crate::error::AppError;
use crate::test_support::{SRC_DIR, stripped_sources};
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

/// The raw call, in the one spelling that can't be dodged: every path form has to
/// name the module (`std::env::current_exe`, or `env::current_exe` under a
/// `use std::env`). The evasion left is a `use std::env::current_exe` and a bare
/// call — which reads identically to the helper at the call site, and is the same
/// hole `ui::file_dialog`'s pin documents for a renaming `use`.
const RAW_CALL: &str = "env::current_exe";

/// The files that may spell it, and **how many times each**. A count rather than a
/// file-level pass, because `desktop_integration` is exactly where a *second* raw
/// call would be the bug this guards — it is the module that writes the `Exec=`
/// line — so forgiving the whole file would pre-authorise the regression. Paths
/// are relative to [`SRC_DIR`].
const EXEMPT: [(&str, usize); 3] = [
    // The helper itself.
    ("services/mod.rs", 1),
    // `is_dev_build`, the one sanctioned reader: it takes `parent()/parent()` and
    // the marker lands on the file name, so it reaches nothing that fn looks at.
    ("services/desktop_integration.rs", 1),
    // This pin, which has to spell the needle to grep for it.
    ("services/tests/mod_tests.rs", 1),
];

/// A floor, so a walk that silently found nothing can't pass vacuously.
const MIN_SOURCES: usize = 200;

/// A test comparing `install_target()` against `services::current_exe()` cannot
/// fail: with no marker in the test process the two agree, so it passes just as
/// well against the raw call it exists to rule out. The routing is only checkable
/// from the corpus — which is the right shape anyway, since what this guards is a
/// *next* call site rather than any existing one, and a marked path is unreachable
/// on any machine a reviewer runs the suite on.
///
/// Its reach is [`SRC_DIR`], which is where a call that matters can live — a
/// binary path is executed, installed to or written down by the app, not by
/// `tests/` or a build script. Two seams it does not cover, both shared with the
/// tree's other corpus pins: `strip_line_comments` handles `//` and not `/* */`,
/// so a block comment naming the call in a non-exempt file would read as one, and
/// the needle is a substring rather than a parse.
#[test]
fn nothing_outside_the_helper_asks_the_os_for_the_binary_path() {
    let mut raw = Vec::new();
    let mut exempt_seen = Vec::new();

    for (path, src) in stripped_sources(SRC_DIR, "rs", MIN_SOURCES) {
        let found = src.matches(RAW_CALL).count();
        match EXEMPT.iter().find(|(exempt, _)| *exempt == path) {
            Some((_, allowed)) => {
                assert_eq!(
                    found, *allowed,
                    "{path} spells `{RAW_CALL}` {found} time(s), not {allowed} — \
                     if that is a new raw call, route it through `services::current_exe`; \
                     if the sanctioned one went away, drop the entry from EXEMPT"
                );
                exempt_seen.push(path);
            }
            None if found > 0 => raw.push(path),
            None => {}
        }
    }

    assert!(
        raw.is_empty(),
        "{raw:?} ask the OS for the binary path directly — use `services::current_exe`, \
         which resolves the `\" (deleted)\"` marker an unlinked executable gets"
    );
    assert_eq!(
        exempt_seen.len(),
        EXEMPT.len(),
        "EXEMPT names {EXEMPT:?} but the walk only reached {exempt_seen:?} — a moved or \
         renamed entry pre-authorises whatever takes its path next"
    );
}

/// The two `Display` shapes in this tree, which is what makes `describe` reachable
/// without knowing which one is in hand. `Network` names an operation and leaves
/// the cause on `.source()`, so the walk is the whole point; `Io` is
/// `#[error("IO error: {0}")]` over the field `#[from]` also makes the source, so
/// a walk that appended unconditionally would print it twice — and sqlx nests that
/// same shape, which is how one constraint failure reached a log line three times
/// over.
#[test]
fn a_cause_is_appended_once_and_never_repeated() {
    let denied = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let cause = denied().to_string();

    let with_context = AppError::network("Failed to parse Deezer response", denied());
    assert_eq!(
        describe(&with_context),
        format!("Network error: Failed to parse Deezer response: {cause}"),
        "a context message drops its cause without the walk"
    );

    let interpolated = AppError::Io(denied());
    assert_eq!(
        describe(&interpolated),
        interpolated.to_string(),
        "a `Display` that already prints its source has nothing left to append"
    );
}
