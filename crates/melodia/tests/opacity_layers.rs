//! No `opacity` draws an element with children through an offscreen texture.
//!
//! Below 1, `0` included, femtovg renders an element that has children into a layer
//! (`Opacity::need_layer`), sized to those children's bounds and allocated again whenever the
//! bounds move, as they do under an easing clip. A childless element takes a canvas alpha instead,
//! so anything with children fades through a `fade` float folded into its brushes.
//!
//! An element's children are read from its own block, from the root of a component it mounts, and
//! for a component's own root, from whatever its mounts hand it: Slint wraps a root's `opacity` at
//! the mount.

use std::collections::{BTreeMap, BTreeSet};

use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, at_own_depth, block_body, head_words, mounted_type, open_braces,
    outside_strings, own_mounts, stripped_sources,
};

/// Items whose children are only those of the block mounting them. A type the tree doesn't declare
/// and this doesn't name is read as bringing children of its own, as a std widget does.
const ITEMS: [&str; 7] =
    ["Rectangle", "Image", "Text", "Path", "TouchArea", "FocusScope", "Flickable"];

/// Compiled out of the item tree, so never anyone's child.
const NOT_ITEMS: [&str; 1] = ["Timer"];

/// The layers kept on purpose, and how many each file holds. Exact, so a second one in any of them
/// is a decision to make rather than one this list already made.
const LAYERED: [(&str, usize); 4] = [
    // Cards of whole panels, size-stable, so each open draws its one texture once.
    ("components/dialog/dialog.slint", 1),
    ("components/onboarding/onboarding-overlay.slint", 1),
    // The selection box: `Checkbox` eases its brushes off its own `fade`, which an eased one stalls.
    ("components/grid/entity-card.slint", 1),
    // Every page passes through it, with no brushes of its own to take a fade.
    ("components/view-transition.slint", 1),
];

/// Loose on purpose: below it the walk has stopped finding the bindings it exists to hold.
const MIN_CHILDLESS: usize = 10;

/// What a block's head declares: the root of a component, or a mount of a type.
enum Head<'a> {
    Root { name: &'a str, base: Option<&'a str> },
    Mount(&'a str),
}

/// A component's root as the tree declares it.
struct Root<'a> {
    body: &'a str,
    base: Option<&'a str>,
}

fn head(src: &str, open: usize) -> Option<Head<'_>> {
    let words = head_words(src, open);
    let Some(at) = words.iter().position(|word| *word == "component") else {
        return mounted_type(src, open).map(Head::Mount);
    };
    let name = words.get(at + 1).copied()?;
    let base = words.iter().position(|word| *word == "inherits");
    Some(Head::Root { name, base: base.and_then(|at| words.get(at + 1)).copied() })
}

/// The components the tree declares, and which of them some mount hands children to.
struct Declared<'a> {
    roots: BTreeMap<&'a str, Root<'a>>,
    /// Appended to the root of a component with no children of its own.
    handed_children: BTreeSet<&'a str>,
}

impl<'a> Declared<'a> {
    fn read(sources: &'a [(String, String)]) -> Self {
        let mut roots = BTreeMap::new();
        let mut handed_children = BTreeSet::new();
        for (_, src) in sources {
            for (open, byte) in outside_strings(src.as_bytes()) {
                if byte != b'{' {
                    continue;
                }
                let Some(body) = block_body(src, open) else { continue };
                match head(src, open) {
                    Some(Head::Root { name, base }) => {
                        let earlier = roots.insert(name, Root { body, base });
                        assert!(
                            earlier.is_none(),
                            "`{name}` is declared twice, and the walk keys components by name, so \
                             one declaration would be read as the other"
                        );
                    }
                    Some(Head::Mount(kind)) if has_children(body) => {
                        handed_children.insert(kind);
                    }
                    _ => {}
                }
            }
        }
        Self { roots, handed_children }
    }

    /// Whether a mount of `kind` has children before its own block adds any.
    fn brings_children(&self, kind: &str) -> bool {
        if ITEMS.contains(&kind) {
            return false;
        }
        self.roots.get(kind).is_none_or(|root| {
            has_children(root.body) || root.base.is_some_and(|base| self.brings_children(base))
        })
    }

    /// Whether the element binding `opacity` at `at` has children, `None` where the block around
    /// the binding declares no element.
    fn lays_a_texture(&self, src: &str, at: usize) -> Option<bool> {
        let open = *open_braces(src, at).last()?;
        let own = has_children(block_body(src, open)?);
        let inherited = match head(src, open)? {
            Head::Root { name, base } => {
                self.handed_children.contains(name)
                    || base.is_some_and(|base| self.brings_children(base))
            }
            Head::Mount(kind) => self.brings_children(kind),
        };
        Some(own || inherited)
    }
}

/// Whether `body` mounts an item at its own depth, the `@children` slot included.
fn has_children(body: &str) -> bool {
    own_mounts(body).any(|(_, kind)| !NOT_ITEMS.contains(&kind))
        || outside_strings(body.as_bytes()).any(|(at, byte)| {
            byte == b'@' && body[at..].starts_with("@children") && at_own_depth(body, at)
        })
}

/// The offset of every `opacity` an element binds itself, by `:` or `<=>`.
fn opacity_bindings(src: &str) -> Vec<usize> {
    src.match_indices("opacity")
        .map(|(at, _)| at)
        .filter(|&at| {
            !src[..at]
                .ends_with(|c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .filter(|&at| {
            let rest = src[at + "opacity".len()..].trim_start();
            rest.starts_with(':') || rest.starts_with("<=>")
        })
        .collect()
}

#[test]
fn no_opacity_draws_an_element_with_children_through_a_texture() {
    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    let declared = Declared::read(&sources);

    let mut childless = 0;
    let mut layered = BTreeMap::new();
    let mut unread = Vec::new();
    for (path, src) in &sources {
        for at in opacity_bindings(src) {
            match declared.lays_a_texture(src, at) {
                Some(true) => *layered.entry(path.as_str()).or_insert(0) += 1,
                Some(false) => childless += 1,
                None => unread.push(format!("{path}:{}", src[..at].lines().count())),
            }
        }
    }

    assert!(unread.is_empty(), "an `opacity` outside any element block the walk reads: {unread:?}");
    assert!(
        childless >= MIN_CHILDLESS,
        "only {childless} `opacity` bindings on childless elements found: the walk went blind"
    );
    assert_eq!(
        layered,
        BTreeMap::from(LAYERED),
        "an `opacity` below 1 on an element with children draws it through an offscreen texture, \
         re-allocated whenever its bounds move. Fold a `fade` float into its brushes, or, where \
         nothing can take one, add the file here with its reason"
    );
}
