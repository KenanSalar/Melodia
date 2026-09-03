//! The workspace's own shape, in the parts nothing else is looking at.
//!
//! Three properties the split rests on and no compile can answer: no member re-exports another,
//! every member sits under the lint table, and every member sits where the corpus walks can
//! reach it. Each is violable from a single line in a `lib.rs` or a `Cargo.toml`, and breaking
//! any of them costs nothing and shows up nowhere — a re-export compiles, a member without
//! `[lints]` compiles, and a member outside `crates/` compiles and takes every walk in
//! `crates/melodia/tests/` quietly out of its own source with it.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use melodia_testkit::{REPO_ROOT, rust_sources};

/// Vacuity floor for the manifest walk, loose for the reason every floor here is: one tight
/// enough to matter would trip on an ordinary member being folded away.
const MIN_MEMBERS: usize = 10;

/// Every member's `Cargo.toml`, paired with the directory name that identifies it.
///
/// Read off `crates/` rather than off the root manifest's `members`, which is a glob —
/// [`every_member_lives_where_the_corpus_walks_can_see_it`] is what holds the two together.
fn member_manifests() -> Vec<(String, String)> {
    let crates = Path::new(REPO_ROOT).join("crates");
    let listing = fs::read_dir(&crates);
    assert!(listing.is_ok(), "`{}` would not list", crates.display());

    let mut manifests = Vec::new();
    let mut unreadable = Vec::new();
    for entry in listing.into_iter().flatten().flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        match fs::read_to_string(&manifest) {
            Ok(text) => manifests.push((name, text)),
            Err(_) => unreadable.push(name),
        }
    }

    // Counted rather than skipped: a manifest that fails to read is a member this walk would
    // otherwise clear without looking at.
    assert!(unreadable.is_empty(), "unreadable member manifests: {unreadable:?}");
    assert!(
        manifests.len() >= MIN_MEMBERS,
        "only {} member manifests found under `crates/`",
        manifests.len()
    );
    manifests.sort();
    manifests
}

/// **No crate re-exports another member's items.**
///
/// A `pub use melodia_x::…` hands a dependent a crate its own manifest was drawn to stop it
/// reaching, which is the whole of what the split buys over a grep: with no re-export anywhere,
/// `melodia_store::database` inside `melodia-views` fails as an unlinked crate rather than as
/// something private, and the second error is the harder one to misread. `pub(crate)` is the
/// weaker fix rather than a middle course, so it is named here too.
///
/// **Two shapes, because the crate prefix is not on the re-export line in either of the
/// interesting ones.** A plain `pub use melodia_x::Thing;` spells it; a `use melodia_x::…::m;`
/// followed by `pub use m::Thing;` does not, and that is the form `melodia-app`'s radio facade
/// shipped — `melodia-views` reached a `melodia-net` constant through it while naming neither.
/// So the names each file binds from a member are collected first, and a re-export whose path
/// starts at one of them counts.
#[test]
fn no_crate_re_exports_another_members_items() {
    let mut offenders = Vec::new();
    for (path, code) in rust_sources() {
        let bound = names_bound_from_members(&code);
        if re_export_heads(&code).any(|head| head.starts_with("melodia_") || bound.contains(head)) {
            offenders.push(path);
        }
    }

    assert!(
        offenders.is_empty(),
        "{offenders:?} re-export another member's items. Every `melodia_*` path is a plain \
         import, so the layer is named at the site that reads it — a re-export hands a dependent \
         a crate its manifest was drawn to keep out of reach"
    );
}

/// Every name a `use melodia_*::…;` in `code` brings into scope: the tail of each path it names,
/// and the alias wherever one is spelled.
///
/// Statements are read to their `;` rather than to the end of a line, a grouped import being the
/// one rustfmt wraps. Braces are treated as separators, so a nested group contributes its inner
/// names too — which only ever widens the set, and the set is what a re-export is checked against.
fn names_bound_from_members(code: &str) -> BTreeSet<&str> {
    let mut bound = BTreeSet::new();
    let mut from = 0;
    while let Some(at) = code[from..].find("use melodia_").map(|rel| rel + from) {
        let path_start = at + "use ".len();
        let Some(len) = code[path_start..].find(';') else {
            break;
        };
        let statement = &code[path_start..path_start + len];
        from = path_start + len;

        for item in statement.split(['{', '}', ',']) {
            let item = item.trim();
            let name = item.rsplit(" as ").next().unwrap_or(item);
            let name = name.rsplit("::").next().unwrap_or(name).trim();
            if !name.is_empty() && !name.starts_with("melodia_") && name != "self" && name != "*" {
                bound.insert(name);
            }
        }
    }
    bound
}

/// The first path segment of every re-export in `code`, in any visibility `pub` can carry.
///
/// `pub(super) use` and `pub(in …) use` are here for the same reason `pub(crate)` is: each one
/// hands the item to somewhere its own manifest does not reach.
fn re_export_heads(code: &str) -> impl Iterator<Item = &str> {
    code.lines().filter_map(|line| {
        let rest = line.trim_start().strip_prefix("pub")?;
        // `pub(crate)`, `pub(super)`, `pub(in path)` — or nothing at all.
        let rest = match rest.strip_prefix('(') {
            Some(scoped) => scoped.split_once(')')?.1,
            None => rest,
        };
        let path = rest.trim_start().strip_prefix("use ")?.trim_start();
        let head = path.split(|c: char| !c.is_alphanumeric() && c != '_').next()?;
        (!head.is_empty()).then_some(head)
    })
}

/// **Every member inherits `[workspace.lints]`.**
///
/// `unwrap_used`, `expect_used` and `dead_code` are held by that table and by nothing else — no
/// corpus walk covers them, because `-D warnings` over the whole workspace already did. A member
/// whose manifest omits the stanza opts out of all three at once, compiles clean, and says
/// nothing about it.
///
/// An equality over the members rather than a floor: a floor cannot see a member stop carrying it.
#[test]
fn every_member_inherits_the_workspace_lints() {
    let missing: Vec<String> = member_manifests()
        .into_iter()
        .filter(|(_, manifest)| !inherits_workspace_lints(manifest))
        .map(|(name, _)| name)
        .collect();

    assert!(
        missing.is_empty(),
        "{missing:?} do not carry `[lints] workspace = true`, so `unwrap_used`, `expect_used` \
         and `dead_code` are unenforced there and nothing else in the build is looking"
    );
}

/// Whether `manifest` takes the workspace lint table, in either spelling TOML allows: a `[lints]`
/// section holding `workspace = true`, or the dotted `lints.workspace = true` at the top level.
fn inherits_workspace_lints(manifest: &str) -> bool {
    let mut in_lints = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_lints = line == "[lints]";
            continue;
        }
        let key_is_set = |key: &str| {
            line.strip_prefix(key)
                .map(str::trim_start)
                .and_then(|rest| rest.strip_prefix('='))
                .is_some_and(|value| value.trim() == "true")
        };
        if key_is_set("lints.workspace") || (in_lints && key_is_set("workspace")) {
            return true;
        }
    }
    false
}

/// **Every member sits directly under `crates/`.**
///
/// `melodia_testkit`'s `rust_source_roots` enumerates `crates/*/src`, so a member declared by any
/// other path contributes nothing to `rust_sources()` — and every walk over it keeps passing,
/// having quietly stopped asking about that member's source. `MIN_SOURCES` cannot catch it
/// either: the tree is large enough that losing a whole crate still clears the floor.
///
/// An equality against the one pattern rather than a prefix test, because the glob is only one
/// level deep: `crates/tools/thing` starts with `crates/` and is still invisible.
#[test]
fn every_member_lives_where_the_corpus_walks_can_see_it() {
    const PATTERN: &str = "crates/*";

    let root = Path::new(REPO_ROOT).join("Cargo.toml");
    let manifest = fs::read_to_string(&root).unwrap_or_default();
    assert!(!manifest.is_empty(), "`{}` would not read", root.display());

    let members = members_array(&manifest);
    assert!(!members.is_empty(), "the root manifest declares no `members`, or the parse broke");

    let elsewhere: Vec<&String> = members.iter().filter(|entry| *entry != PATTERN).collect();
    assert!(
        elsewhere.is_empty(),
        "{elsewhere:?} are workspace members outside `{PATTERN}`. `rust_source_roots` walks \
         `crates/*/src` and only that, so a member reached by any other path drops out of every \
         corpus walk at once, each of which goes on passing"
    );

    // The glob is only half the property. `rust_source_roots` keys on `crates/*/src`, so a member
    // pointing `[lib] path` at a directory of another name satisfies the pattern above and still
    // contributes nothing — and `MIN_SOURCES` cannot see it, the tree being large enough that
    // losing a whole crate clears the floor.
    let sourceless: Vec<String> = member_manifests()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !Path::new(REPO_ROOT).join("crates").join(name).join("src").is_dir())
        .collect();
    assert!(
        sourceless.is_empty(),
        "{sourceless:?} are members with no `src/`, so `rust_sources()` reaches none of their \
         code and every walk over it passes without asking about them"
    );
}

/// The entries of the root manifest's `members = [ … ]`, in whatever shape rustfmt or a hand
/// leaves it: the array is read to its closing bracket rather than to the end of its first line.
fn members_array(manifest: &str) -> Vec<String> {
    let Some(after) = manifest
        .split_once("\nmembers")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.trim_start().strip_prefix('='))
        .and_then(|rest| rest.trim_start().strip_prefix('['))
    else {
        return Vec::new();
    };
    let Some((body, _)) = after.split_once(']') else {
        return Vec::new();
    };
    body.split(',')
        .map(|entry| entry.trim().trim_matches(['"', '\'']).to_owned())
        .filter(|entry| !entry.is_empty())
        .collect()
}
