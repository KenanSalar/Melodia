use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use super::redact_home;
use super::{describe, redact_prefix, undeleted_exe};
use crate::error::AppError;
#[cfg(unix)]
use crate::test_support::with_env_var;
use crate::test_support::{REPO_ROOT, SRC_DIR, font_sources, rel_path, stripped_sources};

/// A Windows home directory, spelled the way `Path::display` spells it. The
/// resolution used to go through `$HOME`, which Windows does not set — so every
/// path in a crash report and in the bundle a user attaches to a public issue
/// shipped the account name verbatim. Nothing on a Linux runner can exercise
/// `dirs::home_dir`'s Windows arm, so the *shape* is pinned here instead.
#[test]
fn a_windows_home_is_redacted_like_a_unix_one() {
    let redacted =
        redact_prefix(r"WARN scan failed for C:\Users\Alice\Music\x.flac", r"C:\Users\Alice");

    assert_eq!(redacted, r"WARN scan failed for ~\Music\x.flac");
}

/// Every occurrence, not just the first: one log line can name a source path and
/// a destination, and a backtrace names the home once per frame.
#[test]
fn every_occurrence_goes() {
    let redacted = redact_prefix("moved /home/alice/a.flac to /home/alice/b.flac", "/home/alice");

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

    let resolved = undeleted_exe(PathBuf::from("/usr/bin/Melodia (deleted)"), |p| p == replacement);

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

/// `licenses/ATTRIBUTION.txt`, which every pin below reads.
const ATTRIBUTION: &str = include_str!("../../../licenses/ATTRIBUTION.txt");

/// The floor for the font walk: the five faces that ship today.
///
/// A floor rather than an exact set, and the asymmetry is the point: it blocks
/// the silent *loss* the walk can't see while permitting the *addition* the pin
/// exists to catch. Retiring a face moves this number deliberately; a
/// subdirectory dropping out of the walk does not, and three of the five sit in
/// one. Tight rather than loose, unlike `test_support`'s source-tree floors —
/// five files changing once a release cycle, where catching *most* of a loss
/// buys nothing.
///
/// Beside the pin rather than `FONTS_DIR`, the opposite of those three: what put
/// them there was four pins duplicating one number. This has one caller and,
/// being tight, asserts about the corpus rather than guarding the walk against
/// vacuity. Move it the day a second pin walks the font tree.
const MIN_FONTS: usize = 5;

/// A face imported by a `.slint` file is `include_bytes!`d into the binary, so it
/// starts being redistributed by all five package formats the moment that import
/// lands — and by the git tree immediately. Neither licence we carry is satisfied
/// by the font's own name table alone: Apache-2.0 §4(a) wants a copy of the
/// licence delivered to recipients, and Material Symbols carries no licence
/// string in its name table at all.
///
/// Asks the *directory* rather than the imports, over-approximating on purpose:
/// a committed face is redistributed by the repo whether or not anything imports
/// it yet, and the import is the edit likeliest to land in a later commit than
/// the file. [`font_sources`] holds the one carve-out.
///
/// Keyed on the repo-relative path, not a family name, the family not being
/// derivable from the file — `MaterialSymbolsRoundedFilled.ttf` declares
/// "Material Symbols Rounded Filled" and no split of the stem gets there without
/// already knowing. The path is exact, and the `Files:` list it checks is what a
/// packager reads anyway.
///
/// **Walk, don't list.** A sixth face is precisely the regression, and a fixed
/// list of the five is what it walks past.
#[test]
fn every_bundled_font_is_named_in_the_attribution() {
    let (fonts, unreadable) = font_sources();
    assert!(unreadable.is_empty(), "unreadable font directories: {unreadable:?}");
    assert!(
        fonts.len() >= MIN_FONTS,
        "only {} .ttf files found under the font tree — a dropped subdirectory \
         silently narrows this pin to whatever is left",
        fonts.len()
    );

    let mut unlicensed = Vec::new();
    for font in &fonts {
        let rel = rel_path(REPO_ROOT, font);
        if !ATTRIBUTION.contains(&rel) {
            unlicensed.push(rel);
        }
    }

    assert!(
        unlicensed.is_empty(),
        "{unlicensed:?} ship inside the binary with nothing in licenses/ATTRIBUTION.txt \
         covering them — add the face under its upstream's entry (or a new entry plus its \
         licence text) and list the file there"
    );
}

/// Which packaging file carries `licenses/`, and the spelling that does it.
///
/// The needle is the mechanism rather than the word: every one of these files can
/// mention the directory in a comment, and a pin that accepts a mention is a pin
/// that goes green on a format which stopped shipping it.
///
/// The RPM stages and ships in separate statements, so the needle has to be the
/// second: `build-rpm.sh` copies `licenses/` into the source tarball, and
/// `%license` is what pulls it out of the unpacked tree. Drop the `%files` line
/// and the staged copy goes unread in the *build* directory, which `check-files`
/// never looks at — no warning, and a pin on the `cp` still green.
const LICENSE_SHIPPERS: [(&str, &str); 5] = [
    ("scripts/build-rpm.sh", "%license LICENSE licenses/"),
    ("Cargo.toml", "[\"licenses/*\""),
    (".github/workflows/release.yml", "cp -r licenses"),
    ("scripts/build-appimage.sh", "cp -r \"$REPO_ROOT/licenses\""),
    ("wix/main.wxs", "$(var.RepoRoot)\\licenses\\"),
];

/// Five formats built by five unrelated toolchains, four of which no reviewer on
/// this machine can run and one of which (the MSI) no Linux runner can build at
/// all. That is exactly the shape where one format quietly stops shipping the
/// licence text and nothing says so until a distro packager files it.
///
/// A `%license` line, an asset triple, a `cp` and an MSI `File` share nothing
/// but their effect, so a named list rather than a walk — the opposite call from
/// the font pin above and for the opposite reason: the set of formats is closed
/// and changing it is deliberate, where the set of fonts is open.
///
/// Naming `release.yml` is why it left `pr-validation.yml`'s skip denylist: it
/// compiles nothing, which was true and became beside the point once it was an
/// input here. A PR touching only that file skipped `test`, and the gate counts
/// `skipped` as a pass.
#[test]
fn every_package_format_ships_the_licenses_dir() {
    let root = Path::new(REPO_ROOT);
    let (mut missing, mut unreadable) = (Vec::new(), Vec::new());

    for (file, needle) in LICENSE_SHIPPERS {
        match std::fs::read_to_string(root.join(file)) {
            Ok(src) if src.contains(needle) => {}
            Ok(_) => missing.push(file),
            Err(_) => unreadable.push(file),
        }
    }

    assert!(
        unreadable.is_empty(),
        "{unreadable:?} are named as shipping licenses/ but won't read — did they move?"
    );
    assert!(
        missing.is_empty(),
        "{missing:?} no longer ship licenses/ — the fonts and the vendored winit fork are \
         compiled into the binary, so every format that ships the binary redistributes \
         them. If the spelling changed rather than the behaviour, update LICENSE_SHIPPERS."
    );
}

/// `src` with every `<!-- … -->` block removed.
///
/// `test_support::strip_line_comments`' argument, in the one markup language that
/// doesn't use `//`: prose about the markup reads exactly like the markup to a pin
/// that greps for a construct. Kept local rather than shared because it has one
/// caller and `main.wxs` is the only XML in the tree a pin reads.
///
/// An unterminated `<!--` swallows the rest of the file, which is the safe
/// direction — the pin then finds nothing and fails, where keeping the tail would
/// silently search markup the compiler never sees either.
fn strip_xml_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(open) = rest.find("<!--") {
        out.push_str(&rest[..open]);
        rest = match rest[open..].find("-->") {
            Some(close) => &rest[open + close + "-->".len()..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// The sibling above catches `main.wxs` dropping `licenses/` altogether. This
/// catches one file going missing from it, which is the likelier half and the one
/// no reviewer here can see: the other four formats glob the directory, so a
/// fourth licence text ships in all of them for free and is absent from exactly
/// the format no Linux runner can build.
///
/// A walk rather than a list, which is [`every_bundled_font_is_named_in_the_attribution`]'s
/// call and not [`every_package_format_ships_the_licenses_dir`]'s — the set of
/// package formats is closed, the set of licence texts is not, and an addition to
/// an open set is precisely what a fixed list walks past.
///
/// Keyed on the file *name* rather than the `Source=` path, so it holds whichever
/// way the attribute is spelled — the `$(var.RepoRoot)` prefix is the sibling's
/// needle and this one has no opinion about it. Which is only safe **because the
/// comments come out first**: `main.wxs`'s own comment names all three files while
/// explaining why each needs a `<File>`, so a needle run over the raw source
/// survives deleting the element it is looking for. Verified by deleting one — the
/// pin passed until [`strip_xml_comments`] went in.
#[test]
fn the_msi_names_every_licence_file() {
    let root = Path::new(REPO_ROOT);
    let raw = std::fs::read_to_string(root.join("wix/main.wxs")).unwrap_or_default();
    assert!(!raw.is_empty(), "wix/main.wxs won't read — did the MSI source move?");
    let wxs = strip_xml_comments(&raw);

    let mut shipped = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("licenses")) {
        for entry in entries.flatten() {
            if entry.path().is_file() {
                shipped.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    assert!(
        !shipped.is_empty(),
        "licenses/ listed no files — a walk that finds nothing satisfies the loop below \
         without ever asking main.wxs for anything"
    );

    let unnamed: Vec<_> = shipped.iter().filter(|name| !wxs.contains(name.as_str())).collect();
    assert!(
        unnamed.is_empty(),
        "{unnamed:?} ship in every other package format but are not `<File>` elements in \
         wix/main.wxs. WiX has no glob, so each file costs an edit there — add one beside \
         the others under `LicenseDir`."
    );
}

/// Windows offers a file type only where `main.wxs` writes the rows for it, and
/// there is no glob there any more than for the licences — so a ninth entry in
/// [`crate::media::AUDIO_EXTENSIONS`] is one the app imports happily and Explorer
/// never offers.
///
/// A walk rather than a list, and comment-stripped first, both for
/// [`the_msi_names_every_licence_file`]'s reasons — the wxs comment names the
/// constant while explaining the pin, satisfying a raw needle by itself.
#[test]
fn the_msi_offers_every_audio_extension() {
    let raw =
        std::fs::read_to_string(Path::new(REPO_ROOT).join("wix/main.wxs")).unwrap_or_default();
    assert!(!raw.is_empty(), "wix/main.wxs won't read — did the MSI source move?");
    let wxs = strip_xml_comments(&raw);

    let unoffered: Vec<_> = crate::media::AUDIO_EXTENSIONS
        .iter()
        .filter(|ext| !wxs.contains(&format!("Name=\".{ext}\"")))
        .collect();
    assert!(
        unoffered.is_empty(),
        "{unoffered:?} are scanned into the library but named nowhere in wix/main.wxs, so \
         Windows never lists Melodia for them in Open with or Default apps. Each needs a row \
         under both `Applications\\Melodia.exe\\SupportedTypes` and `Capabilities\\FileAssociations`."
    );
}

/// `licenses/Vazirmatn-OFL-1.1.txt` and the root `LICENSE`, restated as a DEP-5
/// field body: one leading space per line, trailing whitespace dropped, and a
/// blank line written ` .`.
///
/// The one transformation, so the pin below is its own specification rather than
/// a second opinion about it.
fn as_dep5_field_body(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 40);
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            out.push_str(" .\n");
        } else {
            out.push(' ');
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `usr/share/doc/melodia/copyright` is the first thing a Debian packager reads,
/// and cargo-deb generates it from `license` + `authors` unless handed a file
/// that already opens with DEP-5 keys — which is to say the default states the
/// whole package is AGPL-3.0-or-later by one author, and the bundled fonts and
/// the vendored winit both falsify that.
///
/// Debian Policy 12.5 lets a package *reference* `/usr/share/common-licenses`
/// only for the licences shipped there. Apache-2.0 is one; AGPL-3 and OFL-1.1
/// are not, so both are quoted in full — and a quoted licence is a second copy
/// that can drift from the one the package actually ships. This re-derives both
/// from their sources rather than trusting the copy.
///
/// `LICENSE` is the second file this suite reads that used to sit on
/// `pr-validation.yml`'s skip denylist — see
/// [`every_package_format_ships_the_licenses_dir`] for the argument. An edit to
/// the very file this drift check is anchored on was the one edit it could not
/// run for.
#[test]
fn the_debian_copyright_quotes_the_licences_it_ships() {
    let root = Path::new(REPO_ROOT);
    let copyright =
        std::fs::read_to_string(root.join("packaging/debian-copyright")).unwrap_or_default();

    assert!(
        !copyright.is_empty(),
        "packaging/debian-copyright won't read — cargo-deb falls back to generating \
         `usr/share/doc/melodia/copyright` from `license` + `authors`, which declares the \
         whole package AGPL by one author"
    );
    assert!(
        copyright.starts_with("Format: https://www.debian.org/doc/packaging-manuals/"),
        "packaging/debian-copyright must open with the DEP-5 `Format:` key — without it \
         cargo-deb prepends its own generated header and the stanzas below become a \
         second, contradictory declaration"
    );

    for (source, licence) in [
        ("LICENSE", "AGPL-3.0-or-later"),
        ("licenses/Vazirmatn-OFL-1.1.txt", "OFL-1.1"),
    ] {
        let text = std::fs::read_to_string(root.join(source)).unwrap_or_default();
        assert!(!text.is_empty(), "{source} won't read");
        assert!(
            copyright.contains(as_dep5_field_body(&text).trim_end()),
            "packaging/debian-copyright's `License: {licence}` stanza no longer matches \
             {source}. Regenerate the body with:\n  \
             sed -e 's/[[:space:]]*$//' -e 's/^$/./' -e 's/^/ /' {source}"
        );
    }

    assert!(
        copyright.contains("/usr/share/common-licenses/Apache-2.0"),
        "Apache-2.0 is in Debian's common-licenses, so Policy 12.5 wants a reference to it \
         rather than a third quoted copy"
    );
}

/// A truncated or placeholder copy satisfies every path check above while
/// delivering nothing, and the two texts are large enough that no reviewer diffs
/// them against upstream. One phrase apiece, from deep enough in each to require
/// the body rather than the header.
#[test]
fn the_bundled_licence_texts_are_the_real_ones() {
    let root = Path::new(REPO_ROOT);
    for (file, phrase) in [
        ("licenses/Vazirmatn-OFL-1.1.txt", "SIL OPEN FONT LICENSE Version 1.1"),
        ("licenses/Apache-2.0.txt", "TERMS AND CONDITIONS FOR USE"),
    ] {
        let src = std::fs::read_to_string(root.join(file)).unwrap_or_default();
        assert!(
            src.contains(phrase),
            "{file} does not contain {phrase:?} — missing, truncated, or not the licence \
             text it claims to be"
        );
    }
}
