//! Where Slint's in-window drag may be spelled, and what a mount of it may not do.
//!
//! Each of these builds, survives a quick drag and is wrong: a `DropArea` eating a target's drops,
//! a source that never hears its drag end, a touch gate a binding overrides, an index a rebuilt
//! list never retires, a lift whose texture crops Arabic marks. `slint-pitfalls.md`'s in-window
//! drag entry has the mechanisms.

use std::collections::{BTreeMap, BTreeSet};

use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, at_own_depth, blocks_named, mount_routes, rust_sources,
    stripped_sources,
};

/// Where every route up from a mount ends, the window being the only component nothing mounts.
const ROOT: &str = "export component AppWindow inherits Window";

/// Below this the walks over `DragSource` mounts have stopped finding them. Loose, so folding two
/// grids into one doesn't trip it.
const MIN_DRAG_SOURCES: usize = 6;

/// Every `DropArea` in the tree, each held to its count.
const DROP_AREAS: [(&str, usize); 3] = [
    // The leaf behind every target, keeping the chip on the pointer between them.
    ("app-window.slint", 1),
    ("components/list-drop-target.slint", 1),
    // Sized to the button, where no list sits to take a drop at a slot.
    ("components/now-playing/queue-button.slint", 1),
];

fn sources() -> Vec<(String, String)> {
    stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
}

/// Every file spelling `needle`, with how many times.
fn spellings<'a>(tree: &'a [(String, String)], needle: &str) -> BTreeMap<&'a str, usize> {
    tree.iter()
        .map(|(path, src)| (path.as_str(), src.matches(needle).count()))
        .filter(|(_, count)| *count > 0)
        .collect()
}

/// **A list drags through `DragSource` and nothing else spells `DragArea`.** The source is what
/// carries the touch gate, the `Drag.live` mirror and the click its rows leave, and it sits round a
/// list rather than in a row for the reason [`no_drag_source_is_mounted_inside_a_repeater`] gives.
#[test]
fn only_the_drag_source_spells_drag_area() {
    let tree = sources();

    assert_eq!(
        spellings(&tree, "DragArea"),
        BTreeMap::from([("components/drag-source.slint", 1)]),
        "a list drags through `components/drag-source.slint`'s `DragSource` round its `ListView`"
    );
}

/// **A list takes a drop through `ListDropTarget`, and the two other `DropArea`s are held to their
/// files.** A `Drop` goes to whatever the hit test finds and a `DropArea` takes any it receives, so
/// one drawn over a target eats that target's drops while answering `none` to every move.
#[test]
fn only_the_drop_target_spells_drop_area_beyond_the_two_it_sits_with() {
    let tree = sources();

    assert_eq!(
        spellings(&tree, "DropArea"),
        BTreeMap::from(DROP_AREAS),
        "a list takes drops through `components/list-drop-target.slint`'s `ListDropTarget`: a \
         `DropArea` anywhere else can sit over one and take its drops"
    );
}

/// **No `DragSource` is mounted inside a `for`, whatever file the repeater is in.** The runtime
/// holds a source weakly and a `ListView` destroys a row rather than recycling it, so a source in
/// a row that scrolls out, is rebuilt under the drag or is moved by the reorder itself never hears
/// `drag-finished`, and `Drag.live` stays up until the next press. Climbed rather than read per
/// file: a source moved into a row component sits in no `for` of its own file.
#[test]
fn no_drag_source_is_mounted_inside_a_repeater() {
    let tree = sources();
    let mounts = mount_routes(&tree, "DragSource");
    let mut offenders = Vec::new();

    for (path, routes) in &mounts {
        for route in routes {
            if route.first().map(String::as_str) != Some(ROOT) {
                offenders
                    .push(format!("{path}: climbs to {:?} rather than the window", route.first()));
            }
            if let Some(repeater) = route.iter().find(|head| head.starts_with("for ")) {
                offenders.push(format!("{path}: under `{repeater}`"));
            }
        }
    }

    assert!(
        mounts.len() >= MIN_DRAG_SOURCES,
        "only {} `DragSource` mounts found: the walk is broken",
        mounts.len()
    );
    assert!(
        offenders.is_empty(),
        "a `DragSource` goes round a list, never in a row, or it misses its drag's end:\n{}",
        offenders.join("\n")
    );
}

/// Whether a block's own body binds `enabled`, one way or two.
fn binds_enabled(body: &str) -> bool {
    body.match_indices("enabled").any(|(at, word)| {
        let named = body[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        let bound = body[at + word.len()..].trim_start();
        named && (bound.starts_with(':') || bound.starts_with("<=>")) && at_own_depth(body, at)
    })
}

/// **No `DragSource` mount binds `enabled`.** The source binds it to `!Drag.touching`, which stands
/// every drag down for a touch so the list under a finger pans, and a binding at the mount replaces
/// that rather than joining it. Only a touchscreen shows the difference.
#[test]
fn no_drag_source_mount_overrides_the_touch_gate() {
    let tree = sources();
    let mut mounts = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &tree {
        for body in blocks_named(src, "DragSource") {
            mounts += 1;
            if binds_enabled(body) {
                offenders.push(path.as_str());
            }
        }
    }

    assert!(
        mounts >= MIN_DRAG_SOURCES,
        "only {mounts} `DragSource` mounts found: the walk is broken"
    );
    assert!(
        offenders.is_empty(),
        "a mount binding `enabled` drags under a touch, fighting the pan for the gesture: \
         {offenders:?}"
    );
}

/// The origins the `.slint` tree binds a list to, spelled as Rust's `DragOrigin` variants.
fn armed_origins(tree: &[(String, String)]) -> BTreeSet<String> {
    const BINDING: &str = "origin: DragOrigin.";
    let mut armed = BTreeSet::new();
    for (_, src) in tree {
        for (at, _) in src.match_indices(BINDING) {
            let rest = &src[at + BINDING.len()..];
            let name = rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).next();
            if let Some(name) = name.filter(|name| *name != "none") {
                armed.insert(name.split('-').map(capitalized).collect());
            }
        }
    }
    armed
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// How many sites in the Rust tree revoke each origin.
fn revoked_origins() -> BTreeMap<String, usize> {
    const CALL: &str = "invoke_revoke(DragOrigin::";
    let mut revoked = BTreeMap::new();
    for (_, src) in rust_sources() {
        for (at, _) in src.match_indices(CALL) {
            let rest = &src[at + CALL.len()..];
            let name: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
            *revoked.entry(name).or_insert(0) += 1;
        }
    }
    revoked
}

/// **Every origin a list arms is revoked, at one site, when its rows move.** The drag goes on over
/// the rebuilt rows, the source surviving them, and the index it lifted then names another track,
/// so a reorderable list that forgets its revoke drops onto the wrong row. The origin is a binding
/// in `.slint` and the revoke a call in Rust, so the two drift apart with both files reading clean.
#[test]
fn every_origin_a_list_arms_is_revoked_once() {
    let armed = armed_origins(&sources());
    let once: BTreeMap<String, usize> = armed.iter().map(|origin| (origin.clone(), 1)).collect();

    assert!(!armed.is_empty(), "no mount binds a `DragOrigin`: the walk is broken");
    assert_eq!(
        revoked_origins(),
        once,
        "each list the `.slint` tree arms owes one `invoke_revoke` where its rows move under a \
         drag, and a revoke of an origin nothing arms is stale"
    );
}

/// The identifiers `expr` reads, Slint's `-` included.
fn identifiers(expr: &str) -> impl Iterator<Item = &str> {
    expr.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|word| !word.is_empty())
}

fn reads_a_lift(expr: &str, derived: &[&str]) -> bool {
    identifiers(expr).any(|word| word.contains("lift") || derived.contains(&word))
}

/// The properties `src` declares off a lift, which every row folds into one `fade` its brushes
/// share.
fn lift_derived(src: &str) -> Vec<&str> {
    src.match_indices("property <")
        .filter_map(|(at, _)| {
            let (statement, _) = src[at..].split_once(';')?;
            let (_, declared) = statement.split_once('>')?;
            let (name, binding) = declared.split_once(':')?;
            reads_a_lift(binding, &[]).then_some(name.trim())
        })
        .collect()
}

/// **No row dims its lift through `opacity`.** Below 1 it renders the row into a texture sized to
/// its geometry, which crops the marks above and below an Arabic line for as long as the drag
/// holds it, and a drag of picks lifts a screenful. A row dims through its brushes by
/// `Theme.fade-lifted` instead. Read one property deep, the shape every row has, and invisible to
/// any review on a Latin locale.
#[test]
fn no_row_dims_its_lift_through_opacity() {
    const OPACITY: &str = "opacity:";
    let tree = sources();
    let mut dimming_rows = 0usize;
    let mut offenders = Vec::new();

    for (path, src) in &tree {
        let derived = lift_derived(src);
        if src.contains("Theme.fade-lifted") {
            dimming_rows += 1;
        }
        for (at, _) in src.match_indices(OPACITY) {
            let named = src[..at].ends_with(|c: char| c.is_whitespace() || c == '{');
            let binding = src[at + OPACITY.len()..].split_once(';').map_or("", |(value, _)| value);
            if named && reads_a_lift(binding, &derived) {
                offenders.push(format!("{path}: opacity:{binding}"));
            }
        }
    }

    assert!(dimming_rows >= 2, "only {dimming_rows} rows dim a lift: the walk is broken");
    assert!(
        offenders.is_empty(),
        "a lifted row dims through its brushes, never `opacity`:\n{}",
        offenders.join("\n")
    );
}
