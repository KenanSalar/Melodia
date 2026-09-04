//! Every `paths:` glob in `.claude/rules/` still matches something.
//!
//! It lived in `melodia-testkit` because that crate owns `RULES_DIR`, which put a question about
//! the repository inside the crate every other member dev-depends on.

use melodia_testkit::{REPO_ROOT, RULES_DIR};

/// Every path a `.claude/rules/*.md` `paths:` entry names still matches something.
///
/// A rule loads when Claude *reads* a file its globs match, so a path that has
/// moved doesn't fail loudly — that rule silently stops reaching the code it
/// governs, and the next person to touch that subsystem does so without its
/// contract. Nothing else in the build looks at these files.
///
/// Globs are matched rather than skipped. Skipping them was defensible while a
/// glob could be read as describing a tree that is merely empty for now, but
/// every one in the ruleset names a tree that exists, and a skipped entry is
/// indistinguishable from a rotted one — which is the whole failure this pin is
/// here to catch.
///
/// The frontmatter walk is by hand because it is four lines of one shape and a
/// YAML crate would be a dependency for a test; the *matching* is not, because
/// `**` is exactly the kind of thing that goes subtly wrong when hand-rolled.
#[test]
fn every_path_a_rule_names_still_matches_something() {
    /// Loose enough that adding or retiring a rule doesn't trip it, tight enough
    /// that a frontmatter parse which stopped matching would be caught rather than
    /// passing vacuously. A floor rather than the day's count, which is prose that
    /// needs rewriting on every rule added and was three phases stale when it last
    /// said one.
    const MIN_PATHS: usize = 100;

    let root = std::path::Path::new(RULES_DIR);
    let listing = std::fs::read_dir(root);
    assert!(
        listing.is_ok(),
        "`{RULES_DIR}` would not list — the rules moved, or the anchor is wrong",
    );

    let mut checked = 0_usize;
    let mut missing: Vec<String> = Vec::new();
    let mut malformed: Vec<String> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();

    for entry in listing.into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            unreadable.push(path.display().to_string());
            continue;
        };
        // Frontmatter is the block between the first two `---` lines; a rule
        // without one simply contributes nothing.
        let Some((front, _)) = src.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---"))
        else {
            continue;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();

        for line in front.lines() {
            let Some(glob) = line.trim().strip_prefix("- ") else {
                continue;
            };
            checked += 1;
            // A trailing `/**` means "everything under here" to the rule loader, but `glob`
            // reads it as subdirectories alone — so `licenses/**`, which has none, would look
            // like a rule that had rotted. `/**/*` asks the same question in its dialect.
            // `REPO_ROOT` carries its own trailing separator.
            let pattern = match glob.strip_suffix("/**") {
                Some(dir) => format!("{REPO_ROOT}{dir}/**/*"),
                None => format!("{REPO_ROOT}{glob}"),
            };
            let Ok(matches) = glob::glob(&pattern) else {
                malformed.push(format!("{name}: {glob}"));
                continue;
            };
            if matches.flatten().next().is_none() {
                missing.push(format!("{name}: {glob}"));
            }
        }
    }

    assert!(unreadable.is_empty(), "unreadable rules: {unreadable:?}");
    assert!(malformed.is_empty(), "these rules name unparseable patterns: {malformed:?}");
    assert!(
        missing.is_empty(),
        "these rules name paths that match nothing, so each one has silently stopped \
         loading for the code it governs: {missing:?}",
    );
    assert!(checked >= MIN_PATHS, "only {checked} rule paths found — the frontmatter parse broke");
}
