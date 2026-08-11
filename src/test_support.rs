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

/// The vacuity floor for a walk over [`UI_DIR`], so a traversal that silently found
/// nothing can't pass every pin standing on it.
///
/// Beside the directory it bounds rather than at each caller: the four pins that walk
/// this corpus each carried their own copy of the number, two of them under a comment
/// naming a third as where it came from. Loose on purpose — the tree is well past it, and
/// a floor tight enough to matter would trip on an ordinary file deletion.
///
/// **Not the `SRC_DIR` walks' floor**, which is 200 over a different and much larger tree
/// (`file_dialog_tests`, `services::tests::mod_tests`). Those keep their own `MIN_SOURCES`
/// and should: same name, same purpose, different corpus.
pub(crate) const MIN_SLINT_SOURCES: usize = 100;

/// The root of the Rust tree, for the pins that have to answer "does anything in
/// the tree do X" rather than "do these named files do X" — the native-dialog
/// check being the first, since what it guards against is a *new* call site
/// rather than an edit to a known one.
pub(crate) const SRC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

/// The Rust UI tree, for the pins that ask the same question of every slice's
/// wiring rather than of one subtree.
///
/// Anchored on the manifest dir like its two siblings rather than spelled
/// relative: a bare `"src/ui"` resolves against the harness's working directory,
/// which is the package root only because that is what `cargo test` happens to
/// set.
pub(crate) const UI_SRC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui");

/// The subsystem-contract rules, whose `paths:` frontmatter decides which of
/// them loads for which file.
///
/// Pinned from here because a stale glob fails *silently and invisibly*: the
/// rule simply stops loading for the code it governs, and nothing in the build,
/// the lint gate or the test suite is looking. One `src/ui/` re-home broke four
/// of them at once while updating a fifth.
pub(crate) const RULES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.claude/rules");

/// Every module that owns callback wiring: the cross-cutting root plus the
/// eleven view slices that keep their own.
///
/// **Checked for equality, not containment.** What this guards is a subtree that
/// stops existing — renamed, deleted, or folded somewhere the walk no longer
/// reaches. A floor cannot see that: the walk finds fifty sources where there
/// were fifty-four, every count-based pin over the corpus quietly loses that
/// slice's coverage, and all of them still pass. An exact set turns it into a
/// failing assertion *at the ledger*, naming the home that went missing, rather
/// than a gap somewhere downstream that nothing reports.
pub(crate) const CALLBACK_HOMES: [&str; 12] = [
    "albums",
    "artists",
    "browse",
    // The cross-cutting root: the macros, cross-tab nav, the now-playing
    // fan-out, tags, the updater and library settings — everything that answers
    // to no single view.
    "callbacks",
    "favorites",
    "genres",
    "my_library",
    "playlists",
    "queue_sheet",
    "recently_played",
    "search",
    "tracks",
];

/// A floor under the walk itself, so a traversal that found nothing can't pass
/// vacuously *ahead of* the set check. Loose on purpose — [`CALLBACK_HOMES`] is
/// the real guard, and a floor tight enough to matter would trip on every
/// unrelated file deletion.
const MIN_UI_SOURCES: usize = 180;

/// Every wiring source under [`UI_SRC_DIR`], comment-stripped and paired with its
/// `src/ui`-relative path (`albums/callbacks/lifecycle.rs`,
/// `callbacks/cross_tab_nav.rs`, `queue_sheet/callbacks.rs`).
///
/// A file counts as wiring iff it sits under a `callbacks` *directory* or *is* a
/// `callbacks.rs` — the two shapes the tree uses, a directory once a slice's
/// wiring outgrows one file and a flat file until then. Recognising both is what
/// lets a slice grow from one into the other with no edit here.
///
/// # Panics
///
/// If the set of wiring homes found is not exactly [`CALLBACK_HOMES`], or if
/// [`stripped_sources`]' own floor / unreadable-path checks trip.
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

/// Every file under `root` with extension `ext`, sorted, alongside the
/// directories that wouldn't list.
///
/// The unreadable paths come back rather than being skipped: a dropped subtree
/// lowers whatever a caller counts and its pin goes quiet, and the source-count
/// floors those pins carry are far too loose to notice one missing folder. Every
/// caller asserts the second list is empty.
fn sources_under(root: &str, ext: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>, unreadable: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            unreadable.push(dir.to_path_buf());
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, ext, out, unreadable);
            } else if path.extension().is_some_and(|found| found == ext) {
                out.push(path);
            }
        }
    }

    let (mut sources, mut unreadable) = (Vec::new(), Vec::new());
    walk(Path::new(root), ext, &mut sources, &mut unreadable);
    sources.sort();
    (sources, unreadable)
}

/// Every `.slint` file under [`UI_DIR`], as paths.
///
/// The raw form, for the one pin that reports on the walk itself — the
/// translation check counts the sources it found and names the ones it couldn't
/// read. Anything that only wants the file *contents* wants
/// [`stripped_sources`] instead.
pub(crate) fn slint_sources() -> (Vec<PathBuf>, Vec<PathBuf>) {
    sources_under(UI_DIR, "slint")
}

/// Every source under `root` with extension `ext`, comment-stripped and paired
/// with its `root`-relative path, forward-slashed so a pin can compare against a
/// literal on either platform.
///
/// Shared for the reason [`sources_under`] is, one layer up: both tree-walking
/// pins need this same loop over a different tree, and a copy in each is a copy
/// that can disagree about what "the sources" are.
///
/// # Panics
///
/// If fewer than `floor` files turn up, or any path won't read. The floor is a
/// vacuity guard — a traversal that silently found nothing otherwise passes every
/// pin over it.
pub(crate) fn stripped_sources(root: &str, ext: &str, floor: usize) -> Vec<(String, String)> {
    let (paths, mut unreadable) = sources_under(root, ext);
    assert!(paths.len() >= floor, "only {} .{ext} files found under {root}", paths.len());

    let mut out = Vec::with_capacity(paths.len());
    for path in &paths {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        match fs::read_to_string(path) {
            Ok(src) => out.push((rel, strip_line_comments(&src))),
            Err(_) => unreadable.push(path.clone()),
        }
    }
    assert!(unreadable.is_empty(), "unreadable paths under {root}: {unreadable:?}");
    out
}

/// `src` with everything after an unquoted `//` dropped on each line, keeping the
/// line structure.
///
/// Shared because prose about the code reads exactly like the code to any pin
/// that greps for a construct, and the two that walk the whole tree both trip on
/// it. The translation pin would collect a msgid off the ellipsis placeholders
/// `tab-bar.slint` and `overflow-menu-section.slint` spell inside comments
/// (`@tr("…")`); the scrollbar pin's brace walk would be thrown by any comment
/// quoting an unbalanced `{`.
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

/// Runs of whitespace collapsed to one space, so a pin reads a token sequence rather
/// than one file's indentation.
///
/// Pair it with [`strip_line_comments`] rather than using it alone — this joins lines,
/// so a trailing comment would otherwise run into the code that followed it.
pub(crate) fn normalize_ws(src: &str) -> String {
    src.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The value of a `name:` binding in `src`, up to its terminating `;`, or `""` when
/// `name` doesn't appear.
///
/// The empty string is the caller's failure to report — every pin over this asserts
/// something about the value, and there is no binding whose expected value is nothing.
pub(crate) fn binding_value<'a>(src: &'a str, name: &str) -> &'a str {
    src.split_once(name)
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value)
}

/// The `N` in a global's `out property <int> tab-count: N;`.
///
/// `None` covers both "no such declaration" and "not a plain integer literal", which are
/// one failure to every caller: the count is the sole definition of how many tabs a page
/// has, and Rust clamps the persisted index against it, so anything it can't read is a
/// page that can restore onto a branch mounting nothing.
///
/// Takes the source rather than reading a file, because the two curated globals share
/// one — `RecentlyPlayed`'s pin scopes to its own global's body first, else `Favorites`
/// growing a tab would answer for it.
pub(crate) fn declared_tab_count(src: &str) -> Option<usize> {
    src.split_once("out property <int> tab-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(digits, _)| digits.trim().parse().ok())
}

/// The body of an inline `marker … ];` array literal in `src`.
///
/// The `@tr` arrays a `TabBar` mount hands over have to stay literals — a `[string]`
/// seeded from Rust renders untranslated — so several pins count what is inside one.
pub(crate) fn array_body<'a>(src: &'a str, marker: &str) -> Option<&'a str> {
    src.split_once(marker)
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(body, _)| body)
}

/// The `labels` and `fields` arrays of the one `SortPillRow` mount in `src` whose
/// `sort-field` reads `field_property`, as raw comma-separated element lists.
///
/// `field_property` is the whole property path the mount binds — `Albums.sort-field`,
/// or `Favorites.artist-sort-field` where one global sorts more than one thing. It is
/// the only binding naming both the component and the global, so it locates the mount;
/// the two arrays are then read backwards from it, both being declared above. Returns
/// `None` when no such mount exists, which is itself the failure a caller reports.
///
/// Shared because both sort-pill pins ask the same question of two different view
/// files, and a parser copied into each is a parser that can disagree with itself
/// about what a mount looks like.
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

/// A solid-colour `side` × `side` PNG in a fresh temp dir. The dir is returned
/// alongside the path so the caller can keep it alive — dropping it deletes the
/// file, which is the failure mode to watch for when adopting this.
///
/// Shared because every cover-cache test needs a real decodable image and the
/// two that did each wrote their own; the tier tests want a large source to
/// downscale from and the lookup tests only want *an* image, so the size is the
/// one thing worth parameterising.
pub(crate) fn write_test_png(
    side: u32,
) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("cover.png");
    image::RgbImage::from_pixel(side, side, image::Rgb([120, 60, 200])).save(&path)?;
    Ok((tmp, path))
}

/// A [`Paths`] rooted in a throwaway directory, with the same subdirectories
/// [`Paths::resolve`] creates already in place — so a test that writes into one
/// doesn't have to know which of the fields is a directory.
///
/// Shared rather than hand-rolled per test file because the struct literal names
/// every field, so each copy is one more site to fix when `Paths` grows.
/// Directory creation is best-effort: the caller passed a `TempDir` it just
/// created, and a failure here surfaces as a plain missing-file error in the
/// test body.
pub(crate) fn paths_in(dir: &Path) -> Paths {
    let artwork_dir = dir.join("artwork");
    let artists_dir = dir.join("artists");
    let backups_dir = dir.join("backups");
    let logs_dir = dir.join("logs");
    for sub in [&artwork_dir, &artists_dir, &backups_dir, &logs_dir] {
        let _ = std::fs::create_dir_all(sub);
    }

    Paths {
        data_dir: dir.to_path_buf(),
        db_path: dir.join("melodia.db"),
        settings_path: dir.join("settings.json"),
        view_state_path: dir.join("views.json"),
        queue_path: dir.join("queue.json"),
        search_history_path: dir.join("search_history.json"),
        scrobble_credentials_path: dir.join("scrobble_credentials.json"),
        scrobble_queue_path: dir.join("scrobble_queue.json"),
        scrobble_mbid_state_path: dir.join("scrobble_mbid_attempted.json"),
        artwork_dir,
        artists_dir,
        backups_dir,
        logs_dir,
    }
}

/// Serialises every test in this binary that mutates the process environment,
/// and every test that opts into reading it through [`reading_env`].
///
/// One lock for the whole binary, not one per file or per variable: `cargo
/// test` runs `#[test]` bodies in parallel threads of a single process, the
/// environment is process-global, and glibc's `setenv` may reallocate `environ`
/// out from under another thread's `getenv`. Two tests holding *different*
/// locks are therefore still racing, however careful each is on its own — which
/// is exactly the race Rust 2024 made `set_var`/`remove_var` unsafe for. It
/// lives at the crate root rather than beside any one caller because the set of
/// callers spans unrelated modules and the variables they touch overlap through
/// readers neither of them owns: `SettingsData::default()` reaches
/// `XDG_CURRENT_DESKTOP` via `is_kde_desktop()` *and* all four locale variables
/// via `default_locale()`, and `install_target()` reaches `$APPIMAGE` through
/// `target::current_target_key()`.
///
/// **The read side is opt-in, and that is the limit of what this enforces.**
/// `set_var`'s contract is symmetric — std spells it "no other threads
/// concurrently writing or *reading*(!) the environment" — so a test that only
/// reads is as much a party to the race as a second writer. Nothing makes such a
/// test take a lock; [`reading_env`] is how one opts in, and a reader that
/// hasn't is still racing. Wrap one as soon as you find it rather than assuming
/// this lock already covers it.
///
/// **Private on purpose** — the helpers below are the only way to take it. A
/// `pub(crate)` lock invites a caller to hand-roll the snapshot/restore around
/// it, and three of those had already diverged before they were consolidated
/// here; the restore is the half that goes missing.
static ENV_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// Set while this thread holds [`ENV_LOCK`]. The mutex is not reentrant, so
    /// a nested call would hang the binary with no message and no failing
    /// assertion — the worst failure mode to leave undetected. This turns it
    /// into a named panic instead.
    static ENV_LOCK_HELD: Cell<bool> = const { Cell::new(false) };
}

/// [`ENV_LOCK`] and the reentrancy flag held together, so both are released on
/// the way out of a panicking body as well as a returning one.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// # Panics
    ///
    /// If this thread already holds the lock. Panicking is the point: the
    /// alternative is a silent deadlock.
    fn acquire() -> Self {
        assert!(
            !ENV_LOCK_HELD.get(),
            "the env helpers are not reentrant: this thread already holds the \
             environment lock, so taking it again would deadlock with no message \
             and no failing assertion. A per-variable wrapper must *delegate* to \
             `with_env_set` rather than lock and then call it."
        );
        // A poisoned guard is accepted: the previous holder restored the
        // environment before it resumed unwinding, so the state is consistent.
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

/// Runs `body` with `clear` removed from the environment and `set` applied on
/// top, then restores the originals — including when `body` panics, so a failing
/// assertion can't leak a variable into the rest of the process.
///
/// The whole shape in one place: **lock → snapshot → clear → set →
/// `catch_unwind` → restore → `resume_unwind`.** Every variable in `set` must
/// also appear in `clear`, or there is nothing snapshotted to put it back from.
///
/// Safe to call, and that is encapsulation rather than a gap: this is the only
/// place in the binary that mutates the environment, so "every mutation happens
/// under `ENV_LOCK`" is a property of this module instead of something each
/// caller re-argues.
///
/// # Panics
///
/// Re-raises whatever `body` panicked with, after the environment is restored.
/// Panics up front on a nested call — see [`ENV_LOCK_HELD`].
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

    // SAFETY: `ENV_LOCK` is held across every mutation below *and* across
    // `body`, and the restore runs whether `body` returns or unwinds — so with
    // every other mutation in the binary coming through here too, no writer can
    // overlap another. That discharges the writer half of `set_var`'s contract;
    // the reader half is `reading_env`'s job and is opt-in, which the `ENV_LOCK`
    // doc says plainly rather than letting this comment imply otherwise.
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
/// Named rather than spelled out at each call site because three unrelated test
/// modules override it — `updater::{target,linux_pkg,system_install}::tests` —
/// and it reaches production code none of them owns (`install_target()` →
/// `target::current_target_key()`).
pub(crate) fn with_appimage_env<F: FnOnce() -> R, R>(value: Option<&str>, body: F) -> R {
    with_env_var("APPIMAGE", value, body)
}

/// Runs `body` under the same lock the mutating helpers take, without touching a
/// variable.
///
/// For a test that only *reads* the environment, directly or through production
/// code that does. `set_var`'s contract is symmetric, so such a test races a
/// sibling's mutation exactly as a second mutator would; this is how it opts out
/// of that race. `SettingsData::default()` is the reader in this tree — it
/// reaches `XDG_CURRENT_DESKTOP` and all four locale variables through its serde
/// defaults, and the tests that build one sit in the same file as the tests that
/// mutate both.
pub(crate) fn reading_env<F: FnOnce() -> R, R>(body: F) -> R {
    let _guard = EnvGuard::acquire();
    body()
}

#[cfg(test)]
#[path = "tests/test_support_tests.rs"]
mod tests;
