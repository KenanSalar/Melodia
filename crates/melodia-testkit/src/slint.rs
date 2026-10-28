//! Readers for the shape of a `.slint` file: where a component is mounted, what a global
//! declares, what a binding holds.

use crate::lex::{block_body, normalize_ws, open_braces, strip_line_comments};

/// The body of every `component { … }` block in `src`, braces excluded.
///
/// A declaration is the name followed by `{`, which is what separates `sv := ScrollView {` from
/// the `import { ScrollView } from …` line above it, and the byte before it is checked too, so a
/// longer identifier ending in the same word is not one. Pair it with [`strip_line_comments`] for
/// [`block_body`]'s reason.
pub fn blocks_named<'a>(src: &'a str, component: &str) -> Vec<&'a str> {
    mount_braces(src, component).into_iter().filter_map(|open| block_body(src, open)).collect()
}

/// The `{` of every block [`blocks_named`] reads. A `component X {` definition is not one, where
/// `inherits X {` is: the component it defines is an `X` wherever it is mounted.
fn mount_braces(src: &str, component: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find(component).map(|rel| rel + from) {
        from = at + component.len();
        if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
            continue;
        }
        if src[..at].trim_end().ends_with("component") {
            continue;
        }
        let Some(open) = (from..bytes.len()).find(|i| !bytes[*i].is_ascii_whitespace()) else {
            continue;
        };
        if bytes[open] == b'{' {
            out.push(open);
        }
    }
    out
}

/// Every mount of `component` in `tree`, by path, with every route from it up to a component
/// nothing mounts. A route is the heads of the blocks it passes through, outermost first
/// (`export component Baz inherits Window`, `if cond: Foo`, `for row in rows: Bar`), whitespace
/// collapsed, and ends on the mount's own.
///
/// A route climbs through every place the tree mounts the component enclosing it, so a mount is
/// read inside whatever branch or repeater holds its host, however many files up. Pair it with
/// [`strip_line_comments`] for [`block_body`]'s reason. A head runs back to the nearest `;`, `{`
/// or `}`, so one holding a string that spells one is cut short.
pub fn mount_routes<'a>(
    tree: &'a [(String, String)],
    component: &str,
) -> Vec<(&'a str, Vec<Vec<String>>)> {
    let mut out = Vec::new();
    for (path, src) in tree {
        for open in mount_braces(src, component) {
            out.push((path.as_str(), routes_from(tree, src, open + 1)));
        }
    }
    out
}

/// [`mount_routes`]' climb from byte `at`, which sits inside the block it starts from.
fn routes_from(tree: &[(String, String)], src: &str, at: usize) -> Vec<Vec<String>> {
    let heads = enclosing_heads(src, at);
    let Some(host) = heads.first().and_then(|head| declared_component(head)) else {
        return vec![heads];
    };
    let mut routes = Vec::new();
    for (_, file) in tree {
        for open in mount_braces(file, host) {
            for mut route in routes_from(tree, file, open + 1) {
                route.extend(heads.iter().cloned());
                routes.push(route);
            }
        }
    }
    if routes.is_empty() {
        routes.push(heads);
    }
    routes
}

/// The heads of every block open at byte `at` of `src`, outermost first.
fn enclosing_heads(src: &str, at: usize) -> Vec<String> {
    open_braces(src, at)
        .into_iter()
        .map(|brace| {
            let start = src[..brace].rfind([';', '{', '}']).map_or(0, |boundary| boundary + 1);
            normalize_ws(&src[start..brace])
        })
        .collect()
}

/// The `X` an `export component X …` head declares.
fn declared_component(head: &str) -> Option<&str> {
    let rest = head.strip_prefix("export ").unwrap_or(head).strip_prefix("component ")?;
    rest.split_whitespace().next()
}

/// The `.slint` globals in `src` whose body declares `needle`, by name.
///
/// Declaring a property is what puts a global in the set some dispatcher or installer has to
/// answer for, and two pins ask it of two different properties: `crates/melodia/tests/card_selection.rs`
/// of `selected-ids`, `crates/melodia/tests/cover_generation.rs` of `covers-generation`. Both derive
/// their expected set from this rather than listing one, so a global added to neither the list
/// nor the thing that has to reach it is a miss rather than a pass.
///
/// Pair it with [`strip_line_comments`], [`block_body`]'s own requirement — a `global ` in prose
/// names a global that isn't there.
pub fn globals_declaring(src: &str, needle: &str) -> Vec<String> {
    const KEYWORD: &str = "global ";
    let mut found = Vec::new();
    for (at, _) in src.match_indices(KEYWORD) {
        let head = at + KEYWORD.len();
        let Some(brace) = src.get(head..).and_then(|rest| rest.find('{')) else { continue };
        let name = src.get(head..head + brace).map(str::trim).unwrap_or_default();
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        let Some(body) = block_body(src, head + brace) else { continue };
        if body.contains(needle) {
            found.push(name.to_owned());
        }
    }
    found
}

/// A wrapped condition joined back onto the `if` it belongs to, so a per-line walk sees one
/// statement.
///
/// Slint has no formatter of its own and a branch head carrying more than its own term wraps.
/// A test that only reads first lines counts a wrapped branch as absent, which is the loudest
/// possible way to be wrong about a file that builds.
fn fold_continuations(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("&&") || trimmed.starts_with("||") {
            out.push(' ');
            out.push_str(trimmed);
        } else {
            out.push('\n');
            out.push_str(line);
        }
    }
    out
}

/// The `ViewTransition` body branches `global`'s tabbed page mounts, one line each.
///
/// Comments dropped and continuations folded first, and both terms required: the sheet reads
/// `tab-idx` again for the count line, the placeholder and the pill row, and none of those is a
/// sub-view. Two pins ask about the same branches for different reasons, so the preprocessing is
/// here rather than at either of them, where the pair would be free to drift apart.
pub fn tab_body_branches(sheet: &str, global: &str) -> Vec<String> {
    let tab_term = format!("{global}.tab-idx == {global}.tab-");
    fold_continuations(&strip_line_comments(sheet))
        .lines()
        .filter(|line| line.contains(&tab_term) && line.contains(": ViewTransition {"))
        .map(str::to_owned)
        .collect()
}

/// The value of a `name:` binding in `src`, up to its terminating `;`, or `""` when `name`
/// doesn't appear — the caller's failure to report, there being no binding whose expected
/// value is nothing.
pub fn binding_value<'a>(src: &'a str, name: &str) -> &'a str {
    src.split_once(name).and_then(|(_, rest)| rest.split_once(';')).map_or("", |(value, _)| value)
}

/// The `N` in a global's `out property <int> <property>: N;` — a tabbed page's `tab-count`,
/// or Settings' `category-count`.
///
/// `None` covers both "no such declaration" and "not a plain integer literal": Rust clamps
/// the persisted index against this count, so anything unreadable is a page that can restore
/// onto a branch mounting nothing. Takes the source rather than a path because the two
/// curated globals share one file — a caller scopes to its own global's body first.
pub fn declared_count(src: &str, property: &str) -> Option<usize> {
    src.split_once(&format!("out property <int> {property}:"))
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(digits, _)| digits.trim().parse().ok())
}

/// The body of an inline `marker … ];` array literal in `src`.
///
/// The `@tr` arrays a `TabBar` mount hands over have to stay literals — a `[string]` seeded
/// from Rust renders untranslated — so several pins count what is inside one.
pub fn array_body<'a>(src: &'a str, marker: &str) -> Option<&'a str> {
    src.split_once(marker).and_then(|(_, rest)| rest.split_once("];")).map(|(body, _)| body)
}

/// The `labels` and `fields` arrays of the one sort mount in `src` whose
/// `sort-field` reads `field_property`, as raw comma-separated element lists.
///
/// Either presentation answers to it: `SortPillRow` and `SortMenuPopup` take the same
/// parallel arrays, and the anchor below is a binding both spell.
///
/// `field_property` is the whole property path the mount binds (`Albums.sort-field`, or
/// `Favorites.artist-sort-field` where one global sorts more than one thing) — the only
/// binding naming both the component and the global, so it locates the mount, and the two
/// arrays are read backwards from it. `None` when no such mount exists, which is itself the
/// failure a caller reports.
pub fn sort_mount_arrays<'a>(src: &'a str, field_property: &str) -> Option<(&'a str, &'a str)> {
    let anchor = src.find(&format!("sort-field: {field_property};"))?;
    let head = &src[..anchor];
    let array_after = |start: usize| -> Option<&'a str> {
        let open = src[start..].find('[')? + start + 1;
        let close = src[open..].find(']')? + open;
        Some(&src[open..close])
    };
    Some((array_after(head.rfind("labels:")?)?, array_after(head.rfind("fields:")?)?))
}
