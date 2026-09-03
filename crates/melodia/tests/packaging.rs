//! What the repo ships and what the gate runs: thread-name length, the bundled fonts against
//! their attribution, the five package formats' licence directories, two CI-workflow pins and
//! two MSI ones, the Debian copyright, and the bundled licence texts.
//!
//! None of it asks anything of a crate. Five of these read a file no member owns — a workflow,
//! a shell script, the MSI source — and the other four walk the whole tree, so every one of
//! them was homeless before this crate existed and sat wherever it was written.

use std::path::Path;

use melodia_testkit::{REPO_ROOT, font_sources, rel_path, rust_sources};

/// The setters that spell a thread name: `std::thread::Builder`'s, and the fixed-string form
/// tokio's and rayon's builders share. Anchored on the setter rather than on `Builder::new()`,
/// which two of the call sites leave on the line above.
const NAME_SETTERS: [&str; 2] = [".name(\"", ".thread_name(\""];

/// The setter's closure form, which computes a name per thread instead of spelling one.
const RUNTIME_SETTER: &str = ".thread_name(|";

/// `TASK_COMM_LEN` less its NUL. Written out rather than taken from `libc`, which doesn't export
/// it, and which has nothing to export on the platforms that impose no cap.
const MAX_THREAD_NAME: usize = 15;

/// A floor, so a walk that silently found nothing can't pass vacuously.
const MIN_THREAD_NAMES: usize = 5;

/// The files that compute a name rather than spelling one, where reading the literal measures
/// nothing. Paths are relative to the crate root that holds them.
///
/// One entry rather than two: this pin used to name itself, having to spell the needle it greps
/// for, and out here it is no longer in the corpus it walks.
const RUNTIME_NAMED: [&str; 1] = [
    // `cover-decode-{i}`, whose budget is the prefix plus the widest index the decode pool's
    // clamp can reach; raising that clamp is a thread-name change.
    "media/image/cover_thumbs.rs",
];

/// Linux keeps `TASK_COMM_LEN` bytes of a thread name and std truncates ahead of it rather than
/// erroring, so an over-long name compiles, runs, and is wrong only in `htop`, `perf` and
/// `/proc`: the three places the name exists for, and none of them a place review looks.
///
/// Checkable from the corpus and nowhere else. The truncation happens inside the OS and leaves no
/// value behind, `Thread::name()` still handing back the full string.
///
/// Three seams. Two are shared with the pin above: `strip_line_comments` handles `//` and not
/// `/* */`, and the needle is a substring rather than a parse, so an unrelated builder taking a
/// short literal name would be measured too. That one is harmless while it fits the budget, and a
/// fair prompt to narrow the needle if one ever doesn't. The third is this pin's own:
/// [`RUNTIME_NAMED`] ledgers the computed form of `thread_name`, but `Builder::name` takes any
/// `Into<String>`, so a `format!`ed std thread name matches neither needle and goes unmeasured.
#[test]
fn no_thread_name_outgrows_what_the_kernel_keeps() {
    let mut names = Vec::new();
    let mut computed = Vec::new();

    for (path, src) in rust_sources() {
        for setter in NAME_SETTERS {
            for start in src.match_indices(setter).map(|(at, _)| at + setter.len()) {
                if let Some(len) = src[start..].find('"') {
                    names.push((path.clone(), src[start..start + len].to_owned()));
                }
            }
        }
        if src.contains(RUNTIME_SETTER) {
            computed.push(path);
        }
    }

    let docked: Vec<_> = names.iter().filter(|(_, name)| name.len() > MAX_THREAD_NAME).collect();
    assert!(
        docked.is_empty(),
        "{docked:?} run past the {MAX_THREAD_NAME} bytes Linux keeps, so each arrives docked in \
         every tool that reads it. Shorten them"
    );
    assert!(
        names.len() >= MIN_THREAD_NAMES,
        "only {} thread names found; a renamed setter empties this walk with nothing to see",
        names.len()
    );

    computed.sort();
    assert_eq!(
        computed, RUNTIME_NAMED,
        "the set of files computing a thread name has moved; each one owes its own argument for \
         why the widest name it can produce still fits in {MAX_THREAD_NAME} bytes"
    );
}

/// `licenses/ATTRIBUTION.txt`, which every pin below reads.
const ATTRIBUTION: &str = include_str!("../../../licenses/ATTRIBUTION.txt");

/// The floor for the font walk: the five faces that ship today.
///
/// A floor rather than an exact set, and the asymmetry is the point: it blocks the silent *loss*
/// the walk can't see while permitting the *addition* the pin exists to catch. Retiring a face
/// moves this number deliberately; a subdirectory dropping out of the walk does not, and three of
/// the five sit in one. Tight rather than loose, unlike `test_support`'s source-tree floors — five
/// files changing once a release cycle, where catching *most* of a loss buys nothing.
///
/// Beside the pin rather than `FONTS_DIR` because it has one caller and, being tight, asserts
/// about the corpus rather than guarding the walk against vacuity. Move it the day a second pin
/// walks the font tree.
const MIN_FONTS: usize = 5;

/// A face imported by a `.slint` file is `include_bytes!`d into the binary, so it starts being
/// redistributed by all five package formats the moment that import lands — and by the git tree
/// immediately. Neither licence we carry is satisfied by the font's own name table alone:
/// Apache-2.0 §4(a) wants a copy of the licence delivered to recipients, and Material Symbols
/// carries no licence string in its name table at all.
///
/// Asks the *directory* rather than the imports, over-approximating on purpose: a committed face
/// is redistributed by the repo whether or not anything imports it yet. [`font_sources`] holds the
/// one carve-out.
///
/// Keyed on the repo-relative path, not a family name, the family not being derivable from the
/// file — `MaterialSymbolsRoundedFilled.ttf` declares "Material Symbols Rounded Filled" and no
/// split of the stem gets there without already knowing.
///
/// **Walk, don't list.** A sixth face is precisely the regression, and a fixed list of the five is
/// what it walks past.
#[test]
fn every_bundled_font_is_named_in_the_attribution() {
    let (fonts, unreadable) = font_sources();
    assert!(unreadable.is_empty(), "unreadable font directories: {unreadable:?}");
    assert!(
        fonts.len() >= MIN_FONTS,
        "only {} faces found under the font tree — a dropped subdirectory \
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
/// The needle is the mechanism rather than the word: every one of these files can mention the
/// directory in a comment, and a pin that accepts a mention goes green on a format which stopped
/// shipping it.
///
/// The RPM stages and ships in separate statements, so the needle has to be the second: drop the
/// `%files` line and the staged copy goes unread in the *build* directory, which `check-files`
/// never looks at — no warning, and a pin on the `cp` still green.
/// The MSI source, repo-root-relative. Under the package rather than beside `packaging/`
/// because that is where cargo-wix looks for the wxs of the package it installs, and a flag
/// pointing it elsewhere would be exercised by nothing short of a tagged Windows release.
const MSI_SOURCE: &str = "crates/melodia/wix/main.wxs";

const LICENSE_SHIPPERS: [(&str, &str); 5] = [
    ("scripts/build-rpm.sh", "%license LICENSE licenses/"),
    // The binary's manifest, not the workspace root's: `[package.metadata.deb]` is package
    // metadata and went with the package. Its `../../` is why the needle stops at the glob.
    ("crates/melodia/Cargo.toml", "licenses/*\""),
    ("scripts/build-tarball.sh", "cp -r \"$REPO_ROOT/licenses\""),
    ("scripts/build-appimage.sh", "cp -r \"$REPO_ROOT/licenses\""),
    (MSI_SOURCE, "$(var.RepoRoot)\\licenses\\"),
];

/// Five formats built by five unrelated toolchains, four of which no reviewer on this machine can
/// run and one of which (the MSI) no Linux runner can build at all. That is exactly the shape
/// where one format quietly stops shipping the licence text and nothing says so until a distro
/// packager files it.
///
/// A `%license` line, an asset triple, a `cp` and an MSI `File` share nothing but their effect, so
/// a named list rather than a walk — the opposite call from the font pin above and for the
/// opposite reason: the set of formats is closed and changing it is deliberate, where the set of
/// fonts is open.
///
/// Two of the five carry the same needle from different files, which is the point: the tarball got
/// a `build-tarball.sh` of its own so all five spellings sit beside the format they ship, rather
/// than one of them being a `cp` buried in a matrix slot.
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

/// A job id if `line` is one: under `jobs:`, those are the only keys at exactly one indent
/// carrying no value. The two-space keys above that line (`on:`'s triggers, `permissions:`, `env:`)
/// are either before it or spell a value, so splitting on it is what makes the shape unambiguous.
fn workflow_job_id(line: &str) -> Option<&str> {
    let id = line.strip_prefix("  ")?.strip_suffix(':')?;
    (!id.starts_with([' ', '#'])).then_some(id)
}

fn workflow_job_ids(src: &str) -> Vec<&str> {
    let Some((_, jobs)) = src.split_once("\njobs:\n") else {
        return Vec::new();
    };
    jobs.lines().filter_map(workflow_job_id).collect()
}

/// The `needs:` list of the named job, as spelled in its inline `[a, b, c]` form. Bounded at the
/// next job id, so a job that lost its own `needs:` reads as empty rather than as its neighbour's.
fn workflow_job_needs<'a>(src: &'a str, job: &str) -> Vec<&'a str> {
    let Some((_, body)) = src.split_once(&format!("\n  {job}:\n")) else {
        return Vec::new();
    };
    let Some(list) = body
        .lines()
        .take_while(|line| workflow_job_id(line).is_none())
        .find_map(|line| line.trim_start().strip_prefix("needs:"))
        .and_then(|rest| rest.trim().strip_prefix('[')?.strip_suffix(']'))
    else {
        return Vec::new();
    };
    list.split(',').map(str::trim).filter(|id| !id.is_empty()).collect()
}

/// The aggregate is the required status check, and it can only enforce what it waits on.
///
/// The check step derives its verdict from `toJSON(needs)`, which sees only what `needs:` lists.
/// What that leaves is a job added to the file and never named there: the aggregate doesn't wait
/// for it, so it can report green while that job is still running or already red.
#[test]
fn the_aggregate_waits_on_every_job_in_the_gate_workflow() {
    const AGGREGATE: &str = "pr-validation";

    let path = Path::new(REPO_ROOT).join(".github/workflows/pr-validation.yml");
    let src = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(!src.is_empty(), "unreadable or empty: {}", path.display());

    let jobs = workflow_job_ids(&src);
    assert!(
        jobs.contains(&AGGREGATE) && jobs.len() > 2,
        "parsed {jobs:?} out of {} — the job-id shape changed, not the job list",
        path.display()
    );

    let gated = workflow_job_needs(&src, AGGREGATE);
    assert!(!gated.is_empty(), "`{AGGREGATE}`'s `needs:` list did not parse");

    let ungated: Vec<_> =
        jobs.iter().filter(|id| **id != AGGREGATE && !gated.contains(id)).collect();
    assert!(
        ungated.is_empty(),
        "{ungated:?} are missing from `{AGGREGATE}`'s `needs:`, so the required check never \
         waits on them and goes green whatever they report"
    );
}

/// The value of `key:` in the named job, bounded at the next job id like
/// [`workflow_job_needs`]. Any indent, so a step's `env:` answers as well as the job's.
fn workflow_job_env<'a>(src: &'a str, job: &str, key: &str) -> Option<&'a str> {
    let (_, body) = src.split_once(&format!("\n  {job}:\n"))?;
    let needle = format!("{key}:");
    body.lines()
        .take_while(|line| workflow_job_id(line).is_none())
        .map(str::trim_start)
        .find_map(|line| line.strip_prefix(&needle))
        .map(|value| value.trim().trim_matches(['"', '\'']))
}

/// The gate and the coverage run disagree about debug info, and the disagreement is invisible.
///
/// Each side's reason sits in its own workflow; what neither file can say is that tidying them
/// into agreement reddens nothing. The gate is the half whose backtraces get read, and `0`
/// throws away what `test-windows`' first red run named: `tests\crossfade.rs:152:9`.
///
/// A pin rather than a comment because the comment already failed. The coverage bullet went on
/// describing the gate as keeping *default* debug info for a whole commit after it stopped.
/// `deploy-coverage.yml` leaves the skip denylist for the same reason `LICENSE` is absent from
/// it: compiling nothing is not the same as being unexercised.
#[test]
fn the_two_workflows_disagree_about_debug_info_on_purpose() {
    const KEYS: [&str; 2] = ["CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG"];
    const GATE: [&str; 2] = ["test", "test-windows"];

    let root = Path::new(REPO_ROOT).join(".github/workflows");
    let read = |name: &str| {
        let src = std::fs::read_to_string(root.join(name)).unwrap_or_default();
        assert!(!src.is_empty(), "unreadable or empty: {name}");
        src
    };
    let (gate, coverage) = (read("pr-validation.yml"), read("deploy-coverage.yml"));

    for key in KEYS {
        for job in GATE {
            assert_eq!(
                workflow_job_env(&gate, job, key),
                Some("line-tables-only"),
                "pr-validation.yml's `{job}` must keep `file:line` in its backtraces"
            );
        }
        assert_eq!(
            workflow_job_env(&coverage, "coverage", key),
            Some("0"),
            "deploy-coverage.yml must not pay for DWARF no report reads"
        );
    }

    assert!(
        !gate.contains("!.github/workflows/deploy-coverage.yml"),
        "the skip denylist excludes deploy-coverage.yml again, so the edit that breaks \
         this pin merges with the gate skipped"
    );
}

/// `src` with every `<!-- … -->` block removed.
///
/// `test_support::strip_line_comments`' argument, in the one markup language that doesn't use
/// `//`. Kept local rather than shared because it has one caller and `main.wxs` is the only XML in
/// the tree a pin reads.
///
/// An unterminated `<!--` swallows the rest of the file, which is the safe direction — the pin
/// then finds nothing and fails.
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

/// The sibling above catches `main.wxs` dropping `licenses/` altogether. This catches one file
/// going missing from it, which is the likelier half and the one no reviewer here can see: the
/// other four formats glob the directory, so a fourth licence text ships in all of them for free
/// and is absent from exactly the format no Linux runner can build.
///
/// A walk rather than a list, which is [`every_bundled_font_is_named_in_the_attribution`]'s call
/// and not [`every_package_format_ships_the_licenses_dir`]'s — the set of licence texts is open,
/// and an addition to an open set is precisely what a fixed list walks past.
///
/// Keyed on the file *name* rather than the `Source=` path, so it holds whichever way the
/// attribute is spelled. Which is only safe **because the comments come out first**: `main.wxs`'s
/// own comment names all three files while explaining why each needs a `<File>`, so a needle run
/// over the raw source survives deleting the element it is looking for.
#[test]
fn the_msi_names_every_licence_file() {
    let root = Path::new(REPO_ROOT);
    let raw = std::fs::read_to_string(root.join(MSI_SOURCE)).unwrap_or_default();
    assert!(!raw.is_empty(), "{MSI_SOURCE} won't read — did the MSI source move?");
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

/// The two keys an extension has to appear under, and the reason the pin asks for both rather
/// than for the name anywhere in the file: they feed different lists. `SupportedTypes` is the
/// "Open with" menu, `FileAssociations` is what the Default apps page reads, and an extension
/// written to one of them is offered in exactly half the places a user goes looking.
const MSI_EXTENSION_KEYS: [&str; 2] = [
    r"Software\Classes\Applications\Melodia.exe\SupportedTypes",
    r"Software\Melodia\Capabilities\FileAssociations",
];

/// Windows offers a file type only where `main.wxs` writes the rows for it, and there is no glob
/// there any more than for the licences — so a new entry in [`crate::utils::audio_ext::AUDIO_EXTENSIONS`]
/// is one the app imports happily and Explorer never offers.
///
/// A walk rather than a list, and comment-stripped first, both for
/// [`the_msi_names_every_licence_file`]'s reasons.
#[test]
fn the_msi_offers_every_audio_extension() {
    let raw = std::fs::read_to_string(Path::new(REPO_ROOT).join(MSI_SOURCE)).unwrap_or_default();
    assert!(!raw.is_empty(), "{MSI_SOURCE} won't read — did the MSI source move?");
    let wxs = strip_xml_comments(&raw);

    let mut unoffered = Vec::new();
    for ext in melodia_core::utils::audio_ext::AUDIO_EXTENSIONS {
        for key in MSI_EXTENSION_KEYS {
            if !wxs.contains(&format!("Key=\"{key}\" Name=\".{ext}\"")) {
                unoffered.push(format!(".{ext} under {key}"));
            }
        }
    }
    assert!(
        unoffered.is_empty(),
        "{} are scanned into the library and written nowhere in wix/main.wxs, so Windows \
         leaves Melodia out of Open with, out of Default apps, or out of both. Each extension \
         owes a `<RegistryValue>` under each of the two keys. The needle is one literal, \
         `Key=\"…\" Name=\".ext\"`, so every row failing at once means the attributes were \
         reordered, not deleted.",
        unoffered.join(", ")
    );
}

/// `licenses/Vazirmatn-OFL-1.1.txt` and the root `LICENSE`, restated as a DEP-5 field body: one
/// leading space per line, trailing whitespace dropped, and a blank line written ` .`.
///
/// The one transformation, so the pin below is its own specification rather than a second opinion.
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

/// `usr/share/doc/melodia/copyright` is the first thing a Debian packager reads, and cargo-deb
/// generates it from `license` + `authors` unless handed a file that already opens with DEP-5
/// keys — which is to say the default states the whole package is AGPL-3.0-or-later by one author,
/// and the bundled fonts and the vendored winit both falsify that.
///
/// Debian Policy 12.5 lets a package *reference* `/usr/share/common-licenses` only for the
/// licences shipped there. Apache-2.0 is one; AGPL-3 and OFL-1.1 are not, so both are quoted in
/// full — and a quoted licence is a second copy that can drift from the one the package actually
/// ships. This re-derives both from their sources rather than trusting the copy.
///
/// `LICENSE` compiles nothing and must still stay off `pr-validation.yml`'s skip denylist: a PR
/// touching only that file would skip `test`, and the gate counts `skipped` as a pass.
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

/// A truncated or placeholder copy satisfies every path check above while delivering nothing, and
/// the two texts are large enough that no reviewer diffs them against upstream. One phrase apiece,
/// from deep enough in each to require the body rather than the header.
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
