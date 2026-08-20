//! Test-only helpers shared across the whole crate's unit tests.
//!
//! Gated on `cfg(test)` at the `mod` declaration in `lib.rs` — never compiled
//! into production binaries.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::config::Paths;

/// The root of the Slint tree, for the pins that walk it rather than naming files.
pub(crate) const UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/melodia-ui/ui");

/// Vacuity floor for a walk over [`UI_DIR`], so a traversal that found nothing can't pass
/// every pin standing on it. Loose on purpose — one tight enough to matter would trip on an
/// ordinary file deletion. One of three floors, each bounding a different corpus; none is
/// derivable from another.
pub(crate) const MIN_SLINT_SOURCES: usize = 100;

/// The root of the Rust tree, for the pins answering "does anything in the tree do X" rather
/// than "do these named files do X" — what they guard against is a *new* call site.
pub(crate) const SRC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

/// The bundled font faces, which the Slint build compiles into the binary — so every
/// artifact this repo ships redistributes them and owes their licence text.
pub(crate) const FONTS_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/melodia-ui/ui/assets/fonts");

/// The repo root, for the pins that reach packaging — it lives beside `src/`, not under it.
pub(crate) const REPO_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// The checked-in fixtures, for the tests that need a real file rather than a synthesized one.
/// Anchored on the manifest dir for the reason [`UI_SRC_DIR`] gives.
pub(crate) const ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/assets");

/// The Rust UI tree, for the pins asking the same question of every slice's wiring. Anchored
/// on the manifest dir like its siblings: a bare `"src/ui"` resolves against the harness's
/// working directory, which is the package root only because that is what `cargo test` sets.
pub(crate) const UI_SRC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui");

/// The subsystem-contract rules, whose `paths:` frontmatter decides which loads for which
/// file. Pinned because a stale glob fails *silently* — the rule stops loading for the code
/// it governs and nothing in the build, the lint gate or the test suite is looking.
pub(crate) const RULES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.claude/rules");

/// The unbounded float fed to the guards that reject nonsense input. `f64::from` widens it
/// where the guard under test takes an `f64`.
pub(crate) const UNBOUNDED: f32 = f32::MAX * 2.0;

/// Every module that owns callback wiring: the cross-cutting root plus the twelve view
/// slices that keep their own.
///
/// **Checked for equality, not containment.** What this guards is a subtree that stops
/// existing, which a floor cannot see: every count-based pin over the corpus quietly loses
/// that slice's coverage and all of them still pass.
pub(crate) const CALLBACK_HOMES: [&str; 13] = [
    "albums",
    "artists",
    "browse",
    // The cross-cutting root: everything answering to no single view.
    "callbacks",
    "favorites",
    "genres",
    "my_library",
    "playlists",
    "queue_sheet",
    "radio",
    "recently_played",
    "search",
    "tracks",
];

/// A floor under the walk itself, so it can't pass vacuously *ahead of* the set check.
/// Loose on purpose — [`CALLBACK_HOMES`] is the real guard.
const MIN_UI_SOURCES: usize = 180;

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
pub(crate) fn callback_sources() -> Vec<(String, String)> {
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

/// Every `.slint` file under [`UI_DIR`], as paths. The raw form, for the one pin reporting on
/// the walk itself; anything wanting the file *contents* wants [`stripped_sources`].
pub(crate) fn slint_sources() -> (Vec<PathBuf>, Vec<PathBuf>) {
    sources_under(UI_DIR, "slint")
}

/// What a `.slint` `import` embeds a face from, taken from the compiler's own check
/// (`i-slint-compiler`'s `object_tree.rs`) rather than from what is committed today: the format
/// nothing uses yet is the one that arrives unlicensed.
const FONT_EXTENSIONS: [&str; 3] = ["ttc", "ttf", "otf"];

/// Every shipped face under [`FONTS_DIR`], as paths. The walk recurses, so a face added
/// under a new subdirectory is found with no edit here.
///
/// Every container format the compiler takes, not just the one committed today: an `.otf`'s CFF
/// outlines and a `.ttc`'s several faces reach the same `ttf-parser` a `.ttf` does, and nothing
/// past the `import` cares which arrived. A filter narrower than [`FONT_EXTENSIONS`] is a list
/// wearing a walk's clothes.
///
/// `originals/` is held back, and it is the counterexample to the walk's own premise: Slint
/// embeds a face because a `.slint` file `import`s it, not because it sits under this root,
/// and that directory is gitignored scratch space for the pristine upstream Vazirmatn
/// `scripts/patch_vazirmatn.py` reads.
pub(crate) fn font_sources() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut fonts, unreadable) = sources_under_any(FONTS_DIR, &FONT_EXTENSIONS);
    fonts.retain(|path| !path.components().any(|part| part.as_os_str() == "originals"));
    (fonts, unreadable)
}

/// `path` relative to `root`, forward-slashed so a pin can compare it against a literal on
/// either platform. A path not under `root` comes back whole rather than erroring —
/// reporting the absolute path is more use than a panic if that ever happens.
pub(crate) fn rel_path(root: &str, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string().replace('\\', "/")
}

/// Every source under `root` with extension `ext`, comment-stripped and paired with its
/// `root`-relative path, forward-slashed for comparison against a literal on either platform.
/// Shared for the reason [`sources_under`] is: a copy per walking pin is a copy that can
/// disagree about what "the sources" are.
///
/// # Panics
///
/// If fewer than `floor` files turn up, or any path won't read.
pub(crate) fn stripped_sources(root: &str, ext: &str, floor: usize) -> Vec<(String, String)> {
    let (paths, mut unreadable) = sources_under(root, ext);
    assert!(paths.len() >= floor, "only {} .{ext} files found under {root}", paths.len());

    let mut out = Vec::with_capacity(paths.len());
    for path in &paths {
        let rel = rel_path(root, path);
        match fs::read_to_string(path) {
            Ok(src) => out.push((rel, strip_line_comments(&src))),
            Err(_) => unreadable.push(path.clone()),
        }
    }
    assert!(unreadable.is_empty(), "unreadable paths under {root}: {unreadable:?}");
    out
}

/// `src` with everything after an unquoted `//` dropped on each line, keeping the line
/// structure.
///
/// Prose about the code reads exactly like the code to any pin grepping for a construct: the
/// translation pin would collect a msgid off an `@tr("…")` inside a comment, and the
/// scrollbar pin's brace walk is thrown by any comment quoting an unbalanced `{`.
pub(crate) fn strip_line_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let bytes = line.as_bytes();
        let mut cut = line.len();
        let mut in_string = false;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if in_string => i += 1,
                b'"' => in_string = !in_string,
                b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => {
                    cut = i;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

/// The body of the block whose `{` sits at `open`, braces excluded.
///
/// Quote-aware, and pair it with [`strip_line_comments`] — an unbalanced `{` throws the count
/// whether it sits in a string or in prose. A pin that greps flat instead lets a nested block
/// answer for its parent.
pub(crate) fn block_body(src: &str, open: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open + 1..i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Runs of whitespace collapsed to one space, so a pin reads a token sequence rather than one
/// file's indentation. Pair it with [`strip_line_comments`] — this joins lines, so a trailing
/// comment would otherwise run into the code after it.
pub(crate) fn normalize_ws(src: &str) -> String {
    src.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The value of a `name:` binding in `src`, up to its terminating `;`, or `""` when `name`
/// doesn't appear — the caller's failure to report, there being no binding whose expected
/// value is nothing.
pub(crate) fn binding_value<'a>(src: &'a str, name: &str) -> &'a str {
    src.split_once(name)
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value)
}

/// The `N` in a global's `out property <int> tab-count: N;`.
///
/// `None` covers both "no such declaration" and "not a plain integer literal": Rust clamps
/// the persisted index against this count, so anything unreadable is a page that can restore
/// onto a branch mounting nothing. Takes the source rather than a path because the two
/// curated globals share one file — a caller scopes to its own global's body first.
pub(crate) fn declared_tab_count(src: &str) -> Option<usize> {
    src.split_once("out property <int> tab-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(digits, _)| digits.trim().parse().ok())
}

/// The body of an inline `marker … ];` array literal in `src`.
///
/// The `@tr` arrays a `TabBar` mount hands over have to stay literals — a `[string]` seeded
/// from Rust renders untranslated — so several pins count what is inside one.
pub(crate) fn array_body<'a>(src: &'a str, marker: &str) -> Option<&'a str> {
    src.split_once(marker).and_then(|(_, rest)| rest.split_once("];")).map(|(body, _)| body)
}

/// The `labels` and `fields` arrays of the one `SortPillRow` mount in `src` whose
/// `sort-field` reads `field_property`, as raw comma-separated element lists.
///
/// `field_property` is the whole property path the mount binds (`Albums.sort-field`, or
/// `Favorites.artist-sort-field` where one global sorts more than one thing) — the only
/// binding naming both the component and the global, so it locates the mount, and the two
/// arrays are read backwards from it. `None` when no such mount exists, which is itself the
/// failure a caller reports.
pub(crate) fn sort_pill_row_arrays<'a>(
    src: &'a str,
    field_property: &str,
) -> Option<(&'a str, &'a str)> {
    let anchor = src.find(&format!("sort-field: {field_property};"))?;
    let head = &src[..anchor];
    let array_after = |start: usize| -> Option<&'a str> {
        let open = src[start..].find('[')? + start + 1;
        let close = src[open..].find(']')? + open;
        Some(&src[open..close])
    };
    Some((array_after(head.rfind("labels:")?)?, array_after(head.rfind("fields:")?)?))
}

/// A solid-colour `side` × `side` PNG in a fresh temp dir. The dir comes back alongside the
/// path so the caller can keep it alive — dropping it deletes the file.
pub(crate) fn write_test_png(
    side: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.png");
    image::RgbImage::from_pixel(side, side, image::Rgb([120, 60, 200])).save(&path)?;
    Ok((tmp, path))
}

/// [`write_test_png`]'s counterpart, for the decode paths that only fire on JPEG.
///
/// A gradient rather than a flat fill: a solid colour is one DC coefficient per block, so every
/// scale factor reproduces it exactly and a scaled decode that ignored the size it was asked for
/// would pass anyway.
pub(crate) fn write_test_jpeg(
    side: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    write_test_jpeg_sized(side, side)
}

/// [`write_test_jpeg`] at an arbitrary aspect ratio, for the half of the scaled-decode contract a
/// square source cannot fail: a scale picked off the long edge alone still clears the short one.
pub(crate) fn write_test_jpeg_sized(
    width: u32,
    height: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.jpg");
    let channel = |value: u32| u8::try_from(value % 256).unwrap_or(0);
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([channel(x), channel(y), channel(x + y)])
    })
    .save(&path)?;
    Ok((tmp, path))
}

/// A [`Paths`] rooted in a throwaway directory, with the subdirectories [`Paths::resolve`]
/// creates already in place. Creation is best-effort — a failure surfaces as a missing-file
/// error in the test body.
pub(crate) fn paths_in(dir: &Path) -> Paths {
    let paths = Paths::rooted_at(dir.to_path_buf());
    let _ = paths.create_dirs();
    paths
}

/// Serialises every test in this binary that mutates the process environment, and every test
/// opting into reading it through [`reading_env`]. `.claude/rules/unsafe-rust.md` owns the
/// argument; what binds at the definition is that it is one lock for the whole *binary* —
/// two tests holding different locks are still racing — that the read side is opt-in, so a
/// reader that hasn't wrapped itself is still racing, and that it stays private, a
/// `pub(crate)` lock inviting the hand-rolled snapshot/restore whose restore goes missing.
static ENV_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// Set while this thread holds [`ENV_LOCK`]. The mutex is not reentrant, so a nested
    /// call would hang the binary with no message; this turns that into a named panic.
    static ENV_LOCK_HELD: Cell<bool> = const { Cell::new(false) };
}

/// [`ENV_LOCK`] and the reentrancy flag held together, so both release on the way out of a
/// panicking body as well as a returning one.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// # Panics
    ///
    /// If this thread already holds the lock — the alternative is a silent deadlock.
    fn acquire() -> Self {
        assert!(
            !ENV_LOCK_HELD.get(),
            "the env helpers are not reentrant: this thread already holds the \
             environment lock, so taking it again would deadlock with no message \
             and no failing assertion. A per-variable wrapper must *delegate* to \
             `with_env_set` rather than lock and then call it."
        );
        // Poison is accepted — the previous holder restored the environment before unwinding.
        let lock = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        ENV_LOCK_HELD.set(true);
        Self { _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        ENV_LOCK_HELD.set(false);
    }
}

/// Runs `body` with `clear` removed from the environment and `set` applied on top, then
/// restores the originals — including when `body` panics, so a failing assertion can't leak a
/// variable into the rest of the process.
///
/// Every variable in `set` must also appear in `clear`, or there is nothing snapshotted to
/// put it back from. Safe to call, which is encapsulation rather than a gap: this being the
/// binary's only mutation site, "every mutation happens under `ENV_LOCK`" is a property of
/// the module rather than something each caller re-argues.
///
/// # Panics
///
/// Re-raises whatever `body` panicked with, after the environment is restored. Panics up
/// front on a nested call — see [`ENV_LOCK_HELD`].
#[allow(
    unsafe_code,
    reason = "env::set_var/remove_var are unsafe in Rust 2024; every mutation in the test binary happens in this function, under ENV_LOCK, restored under catch_unwind."
)]
pub(crate) fn with_env_set<F: FnOnce() -> R, R>(
    clear: &[&str],
    set: &[(&str, &str)],
    body: F,
) -> R {
    debug_assert!(
        set.iter().all(|(var, _)| clear.contains(var)),
        "a variable in `set` that isn't in `clear` is never restored",
    );

    let _guard = EnvGuard::acquire();
    let saved: Vec<(&str, Option<String>)> =
        clear.iter().map(|&v| (v, std::env::var(v).ok())).collect();

    // SAFETY: `ENV_LOCK` is held across every mutation below *and* across `body`, and the
    // restore runs whether `body` returns or unwinds — so with every other mutation in the
    // binary coming through here too, no writer can overlap another. That discharges the
    // writer half of `set_var`'s contract; the reader half is `reading_env`'s, and opt-in.
    unsafe {
        for &v in clear {
            std::env::remove_var(v);
        }
        for (var, value) in set {
            std::env::set_var(var, value);
        }
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));

    // SAFETY: as above — still the same guard, still the same lock.
    unsafe {
        for (var, value) in saved {
            match value {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
    }

    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Runs `body` with `var` set to `value`, or merely cleared when it is `None`.
pub(crate) fn with_env_var<F: FnOnce() -> R, R>(var: &str, value: Option<&str>, body: F) -> R {
    match value {
        Some(v) => with_env_set(&[var], &[(var, v)], body),
        None => with_env_set(&[var], &[], body),
    }
}

/// Runs `body` with `$APPIMAGE` set to `value`, or cleared when it is `None`.
///
/// Named rather than spelled at each call site because three unrelated test modules override
/// it and it reaches production code none of them owns (`install_target()`).
pub(crate) fn with_appimage_env<F: FnOnce() -> R, R>(value: Option<&str>, body: F) -> R {
    with_env_var("APPIMAGE", value, body)
}

/// Runs `body` under the same lock the mutating helpers take, without touching a
/// variable.
///
/// For a test that only *reads* the environment, directly or through production code that
/// does: `set_var`'s contract is symmetric, so such a test races a sibling's mutation exactly
/// as a second mutator would. `SettingsData::default()` is the reader in this tree, reaching
/// `XDG_CURRENT_DESKTOP` and all four locale variables through its serde defaults.
pub(crate) fn reading_env<F: FnOnce() -> R, R>(body: F) -> R {
    let _guard = EnvGuard::acquire();
    body()
}

/// The home directory `services::redact_home` will actually resolve, if there is one.
///
/// A redaction test builds its fixture from this rather than spelling `/home/testuser` under a
/// faked `$HOME`: `dirs::home_dir()` asks Win32 for the profile folder and never reads the
/// environment, so the faked form is a fixture only Linux can honour and every such test was
/// red on Windows. `None` leaves the caller nothing to assert against.
///
/// Deliberately the production resolver rather than a second copy of it. A test that resolved
/// home its own way would agree with `redact_home` only by coincidence, and the coincidence
/// breaks on whichever platform the two happen to disagree about — which is the entire class
/// of bug this helper exists because of. The Unix `$HOME` arm is pinned separately, by
/// `services::tests::mod_tests`, which is where a faked variable still belongs.
pub(crate) fn resolved_home() -> Option<String> {
    crate::services::home_dir_string()
}

#[cfg(test)]
#[path = "tests/test_support_tests.rs"]
mod tests;
