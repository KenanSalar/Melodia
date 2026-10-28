//! Corpus walks: every source of one kind under a root, read whole, with the floors that keep a
//! walk from passing on nothing.

use std::fs;
use std::path::{Path, PathBuf};

use crate::lex::strip_line_comments;
use crate::{FONTS_DIR, REPO_ROOT, UI_SRC_DIR};

/// Vacuity floor for a walk over [`UI_DIR`](crate::UI_DIR), so a traversal that found nothing
/// can't pass every pin standing on it. Loose on purpose — one tight enough to matter would trip
/// on an ordinary file deletion. One of three floors, each bounding a different corpus; none is
/// derivable from another.
pub const MIN_SLINT_SOURCES: usize = 100;

/// Vacuity floor for a walk over the Rust tree, loose for [`MIN_SLINT_SOURCES`]' reason.
/// Shared rather than declared per pin: every walk over the Rust tree stands on it, and a copy
/// apiece is a copy that can disagree about how much of it has to be there.
pub const MIN_SOURCES: usize = 200;

/// Every module that owns callback wiring: the cross-cutting root plus the twelve view
/// slices that keep their own.
///
/// **Checked for equality, not containment.** What this guards is a subtree that stops
/// existing, which a floor cannot see: every count-based pin over the corpus quietly loses
/// that slice's coverage and all of them still pass.
pub const CALLBACK_HOMES: [&str; 14] = [
    "albums",
    "artists",
    "browse",
    // The cross-cutting root: everything answering to no single view.
    "callbacks",
    "favorites",
    "genres",
    "my_library",
    "onboarding",
    "playlists",
    "queue_panel",
    "radio",
    "recently_played",
    "search",
    "tracks",
];

/// A floor under any walk of [`UI_SRC_DIR`], so one can't pass vacuously. Loose on purpose —
/// each caller has a real guard of its own, [`CALLBACK_HOMES`] here.
pub const MIN_UI_SOURCES: usize = 180;

/// Every wiring source under [`UI_SRC_DIR`], comment-stripped and paired with its
/// `src/ui`-relative path.
///
/// A file counts as wiring iff it sits under a `callbacks` *directory* or *is* a
/// `callbacks.rs` — recognising both is what lets a slice grow from one into the other with
/// no edit here.
///
/// # Panics
///
/// If the set of homes found is not exactly [`CALLBACK_HOMES`], or [`stripped_sources`]' own
/// checks trip.
pub fn callback_sources() -> Vec<(String, String)> {
    use std::collections::BTreeSet;

    let mut found = BTreeSet::new();
    let mut out = Vec::new();

    for (rel, code) in stripped_sources(UI_SRC_DIR, "rs", MIN_UI_SOURCES) {
        let mut parts = rel.split('/');
        let Some(home) = parts.next() else { continue };
        let is_wiring = home == "callbacks"
            || parts.next().is_some_and(|p| p == "callbacks" || p == "callbacks.rs");
        if !is_wiring {
            continue;
        }
        found.insert(home.to_owned());
        out.push((rel, code));
    }

    let expected: BTreeSet<String> = CALLBACK_HOMES.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(
        found, expected,
        "the set of callback homes under {UI_SRC_DIR} no longer matches `CALLBACK_HOMES`. A \
         *missing* entry is wiring that was deleted or renamed — every pin walking this corpus \
         just lost that slice's coverage with nothing to report it. An *extra* entry is a new \
         wiring home no pin is checking yet: add it to the ledger."
    );

    out
}

/// Every file under `root` with extension `ext`, sorted, alongside the directories that
/// wouldn't list.
///
/// Unreadable paths come back rather than being skipped — a dropped subtree lowers whatever
/// a caller counts and its pin goes quiet, the floors being far too loose to notice one
/// missing folder. Every caller asserts the second list is empty.
fn sources_under(root: &str, ext: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    sources_under_any(root, &[ext])
}

/// [`sources_under`] over a set of extensions, in one pass. A walk apiece would report an
/// unlistable directory once per extension and hand back sorted halves whose concatenation
/// isn't sorted.
fn sources_under_any(root: &str, exts: &[&str]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>, unreadable: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            unreadable.push(dir.to_path_buf());
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, exts, out, unreadable);
            } else if path.extension().is_some_and(|found| exts.iter().any(|ext| found == *ext)) {
                out.push(path);
            }
        }
    }

    let (mut sources, mut unreadable) = (Vec::new(), Vec::new());
    walk(Path::new(root), exts, &mut sources, &mut unreadable);
    sources.sort();
    (sources, unreadable)
}

/// What a `.slint` `import` embeds a face from, taken from the compiler's own check
/// (`i-slint-compiler`'s `object_tree.rs`) rather than from what is committed today: the format
/// nothing uses yet is the one that arrives unlicensed.
const FONT_EXTENSIONS: [&str; 3] = ["ttc", "ttf", "otf"];

/// Every shipped face under [`FONTS_DIR`], as paths. The walk recurses, so a face added
/// under a new subdirectory is found with no edit here.
///
/// Every container format the compiler takes, not just the one committed today: an `.otf`'s CFF
/// outlines and a `.ttc`'s several faces reach the same `skrifa` a `.ttf` does, and nothing
/// past the `import` cares which arrived. A filter narrower than [`FONT_EXTENSIONS`] is a list
/// wearing a walk's clothes.
///
/// `originals/` is held back, and it is the counterexample to the walk's own premise: Slint
/// embeds a face because a `.slint` file `import`s it, not because it sits under this root,
/// and that directory is gitignored scratch space for the pristine upstream faces
/// `scripts/patch_vazirmatn.py` reads.
pub fn font_sources() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut fonts, unreadable) = sources_under_any(FONTS_DIR, &FONT_EXTENSIONS);
    fonts.retain(|path| !path.components().any(|part| part.as_os_str() == "originals"));
    (fonts, unreadable)
}

/// `path` relative to `root`, forward-slashed so a pin can compare it against a literal on
/// either platform. A path not under `root` comes back whole rather than erroring —
/// reporting the absolute path is more use than a panic if that ever happens.
pub fn rel_path(root: &str, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string().replace('\\', "/")
}

/// Every source under `root` with extension `ext`, paired with its `root`-relative path,
/// forward-slashed for comparison against a literal on either platform. Shared for the reason
/// [`sources_under`] is: a copy per walking pin is a copy that can disagree about what "the
/// sources" are.
///
/// Comments included, for the pin that reads them. Anything grepping for a construct wants
/// [`stripped_sources`].
///
/// # Panics
///
/// If fewer than `floor` files turn up, or any path won't read.
pub fn raw_sources(root: &str, ext: &str, floor: usize) -> Vec<(String, String)> {
    let (paths, mut unreadable) = sources_under(root, ext);
    assert!(paths.len() >= floor, "only {} .{ext} files found under {root}", paths.len());

    let mut out = Vec::with_capacity(paths.len());
    for path in &paths {
        let rel = rel_path(root, path);
        match fs::read_to_string(path) {
            Ok(src) => out.push((rel, src)),
            Err(_) => unreadable.push(path.clone()),
        }
    }
    assert!(unreadable.is_empty(), "unreadable paths under {root}: {unreadable:?}");
    out
}

/// [`raw_sources`] with every line comment dropped.
///
/// # Panics
///
/// As [`raw_sources`].
pub fn stripped_sources(root: &str, ext: &str, floor: usize) -> Vec<(String, String)> {
    raw_sources(root, ext, floor)
        .into_iter()
        .map(|(path, src)| (path, strip_line_comments(&src)))
        .collect()
}

/// Every crate's source root, paired with the directory name that identifies it.
///
/// "The Rust tree" is one walk per crate now, so the pins answering *does anything in the tree
/// do X* need the set rather than a path. Every package is under `crates/`, the binary
/// included, so the workspace root itself contributes nothing.
fn rust_source_roots() -> Vec<(String, PathBuf)> {
    let repo = Path::new(REPO_ROOT);
    let mut roots = Vec::new();

    if let Ok(entries) = fs::read_dir(repo.join("crates")) {
        for entry in entries.flatten() {
            let src = entry.path().join("src");
            if src.is_dir() {
                roots.push((entry.file_name().to_string_lossy().into_owned(), src));
            }
        }
    }

    roots.sort();
    assert!(!roots.is_empty(), "no Rust source root under {REPO_ROOT}");
    roots
}

/// Every Rust source in the tree, comment-stripped and paired with the path it came from.
///
/// # Panics
///
/// As [`raw_rust_sources`].
pub fn rust_sources() -> Vec<(String, String)> {
    raw_rust_sources().into_iter().map(|(path, src)| (path, strip_line_comments(&src))).collect()
}

/// Every Rust source in the tree as written, for a walk that lexes comments itself:
/// [`strip_line_comments`] works a line at a time, so it cuts a `//` inside a string literal that
/// continued from the line above.
///
/// Paths are relative to the crate root that produced them, and the workspace layout is what
/// makes that unambiguous: each crate holds the `src/`-relative subtree it owned when this was
/// one crate, so `utils/exe.rs` is still `utils/exe.rs` whichever crate it ended up in, and
/// every exemption table naming one reads the same. Files sitting *directly* in a crate's
/// `src/` are the exception, every crate having a `lib.rs`, so those take their crate's
/// directory name in front.
///
/// # Panics
///
/// If fewer than [`MIN_SOURCES`] files turn up across all roots, or any path won't read.
pub fn raw_rust_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut unreadable = Vec::new();

    for (krate, root) in rust_source_roots() {
        let root = root.display().to_string();
        let (paths, mut missed) = sources_under(&root, "rs");
        unreadable.append(&mut missed);
        for path in &paths {
            let rel = rel_path(&root, path);
            let rel = if rel.contains('/') { rel } else { format!("{krate}/{rel}") };
            match fs::read_to_string(path) {
                Ok(src) => out.push((rel, src)),
                Err(_) => unreadable.push(path.clone()),
            }
        }
    }

    assert!(unreadable.is_empty(), "unreadable paths in the Rust tree: {unreadable:?}");
    assert!(out.len() >= MIN_SOURCES, "only {} .rs files found in the Rust tree", out.len());
    out.sort();
    out
}

/// Every file in the Rust tree that spells `needle`, minus the ones `exempt` names — each of
/// which is held to its exact count rather than forgiven wholesale, since a second call written
/// into a sanctioned file is itself the regression.
///
/// Shared rather than per-pin because the walks standing on it live in different modules, and a
/// copy apiece is a copy that can drift on the exemption semantics, the half a reviewer cannot
/// check by reading one of them.
///
/// Two seams it does not cover, both shared with the tree's other corpus pins:
/// `strip_line_comments` handles `//` and not `/* */`, and the needle is a substring rather
/// than a parse.
pub fn spellings_outside(needle: &str, exempt: &[(&str, usize)]) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut offenders = Vec::new();
    // A set rather than a tally: paths are crate-`src/`-relative now, so two crates can hand back
    // one key, and counting matches would let a doubled hit stand in for an entry matching nothing.
    let mut exempt_seen = BTreeSet::new();

    for (path, src) in rust_sources() {
        let found = src.matches(needle).count();
        match exempt.iter().find(|(name, _)| *name == path) {
            Some((_, allowed)) => {
                assert_eq!(
                    found, *allowed,
                    "{path} spells `{needle}` {found} time(s), not {allowed} — either route the \
                     new one through the shared helper, or drop the stale entry from the list"
                );
                exempt_seen.insert(path);
            }
            None if found > 0 => offenders.push(path),
            None => {}
        }
    }

    assert_eq!(
        exempt_seen.len(),
        exempt.len(),
        "the exemptions for `{needle}` name {exempt:?} but the walk only reached \
         {exempt_seen:?} — a moved or renamed entry pre-authorises whatever takes its path next"
    );
    offenders
}
