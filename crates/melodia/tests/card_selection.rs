//! The card grids' multi-selection, in the places no compile can reach it.
//!
//! Every rule here is violable from a file that builds and looks right, and every one of them
//! fails the same way: a set the user cannot see, on a grid where the next click picks instead of
//! opening. `.claude/rules/ui-patterns.md` argues each; this is what holds them.

use std::collections::BTreeSet;

use melodia_testkit::{
    MIN_SLINT_SOURCES, MIN_UI_SOURCES, UI_DIR, UI_SRC_DIR, block_after, depth_between,
    globals_declaring, stripped_sources,
};

/// The six section leaves that hand the shared card selection back. **An equality, not a floor**:
/// one slice quietly dropping its `clear()` is the regression, and a floor cannot see it. Browse
/// and Tracks are deliberately absent, each keeping its row model across the leave and clearing
/// its own `selected-ids` instead.
const CARD_SELECTION_LEAVES: [&str; 6] = [
    "albums/callbacks/lifecycle.rs",
    "artists/callbacks/lifecycle.rs",
    "favorites/callbacks/lifecycle.rs",
    "genres/callbacks/lifecycle.rs",
    "playlists/callbacks/lifecycle.rs",
    "recently_played/callbacks/lifecycle.rs",
];

/// Every `.slint` file that may name `CardSelection` at all: the global, the one cell that derives
/// a scope's answers, the Escape dispatcher, and the root's import plus export list.
const CARD_SELECTION_HOMES: [&str; 4] = [
    "app-window.slint",
    "components/grid/card-cell.slint",
    "globals/card-selection.slint",
    "globals/selection.slint",
];

/// The five grids that can fit on one row, each reading the count off whichever model its mounted
/// tab drew.
const LONE_ROW_MOUNTS: [&str; 5] = [
    "views/browse-view.slint",
    "views/favorites-view.slint",
    "views/my-library-view.slint",
    "views/radio-view.slint",
    "views/recently-played-view.slint",
];

/// The queue sheet owns Escape for as long as it is up and clears its own selection on the first
/// press, so joining the dispatcher would put a second arm in front of that one.
const NOT_IN_THE_DISPATCHER: &str = "Queue";

fn wiring_sources() -> Vec<(String, String)> {
    stripped_sources(UI_SRC_DIR, "rs", MIN_UI_SOURCES)
        .into_iter()
        .filter(|(rel, _)| !rel.split('/').any(|segment| segment == "tests"))
        .collect()
}

fn slint_sources() -> Vec<(String, String)> {
    stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
}

/// Whether `needle` sits inside the block that opens after `opener`, rather than merely further
/// down the same file.
fn inside_block_after(src: &str, opener: &str, needle: &str) -> bool {
    let Some(at) = src.find(opener) else { return false };
    let Some(open) = src.get(at..).and_then(|rest| rest.find('{')).map(|rel| at + rel) else {
        return false;
    };
    src.match_indices(needle)
        .any(|(found, _)| depth_between(src, open, found).is_some_and(|depth| depth >= 1))
}

/// What a global declares to be one of the surfaces the Escape arm has to know about.
const SELECTION_MODEL: &str = "property <[int]> selected-ids";

/// The globals a `Selection` body asks about, read off the `X.selected-ids.length > 0` each of its
/// arms is written as.
fn surfaces_tested_in(body: &str) -> BTreeSet<String> {
    const TEST: &str = ".selected-ids.length > 0";
    body.match_indices(TEST)
        .filter_map(|(at, _)| {
            let head = body.get(..at)?;
            let start = head.rfind(|c: char| !c.is_alphanumeric() && c != '_')? + 1;
            head.get(start..).map(str::to_owned)
        })
        .collect()
}

#[test]
fn every_section_leave_hands_its_card_selection_back() {
    let sources = wiring_sources();
    let mut gates = 0_usize;
    let mut clearing: BTreeSet<&str> = BTreeSet::new();

    for (path, src) in &sources {
        if !src.contains("on_section_active_changed(") {
            continue;
        }
        gates += 1;
        if inside_block_after(src, "on_section_active_changed(", "CardSelection>().invoke_clear()")
        {
            clearing.insert(path.as_str());
        }
    }

    assert!(gates >= 10, "only {gates} section gates found, so the walk is broken");
    assert_eq!(
        clearing,
        CARD_SELECTION_LEAVES.into_iter().collect::<BTreeSet<&str>>(),
        "the shared card selection is one global keyed by one scope string, so a leave that stops \
         clearing it leaves a set no grid is showing: on re-entry every click picks instead of \
         opening, and there is no pill and no second activation to undo it with"
    );
}

#[test]
fn a_card_grid_mount_states_its_scope_and_nothing_else() {
    let sources = slint_sources();
    let naming: BTreeSet<&str> = sources
        .iter()
        .filter(|(_, src)| src.contains("CardSelection"))
        .map(|(path, _)| path.as_str())
        .collect();

    assert_eq!(
        naming,
        CARD_SELECTION_HOMES.into_iter().collect::<BTreeSet<&str>>(),
        "`CardCell` derives `selected`, `selection-live`, the menu's effective selection and its \
         count from one scope; a mount reaching past them re-spells what the cell already answers, \
         and the two spellings drift. `BrowseCardGrid` is the exemption and reads \
         `Browse.selected-ids` instead, so it must not appear here either"
    );
}

#[test]
fn every_multi_selection_surface_is_one_the_escape_arm_asks_about() {
    let sources = slint_sources();
    let declaring: BTreeSet<String> =
        sources.iter().flat_map(|(_, src)| globals_declaring(src, SELECTION_MODEL)).collect();

    let dispatcher = sources
        .iter()
        .find(|(path, _)| path == "globals/selection.slint")
        .map_or("", |(_, src)| src.as_str());
    let asked = surfaces_tested_in(block_after(dispatcher, "function any-live()"));
    let cleared = surfaces_tested_in(block_after(dispatcher, "function clear-live()"));

    assert!(
        declaring.len() >= 10,
        "only {} selection globals found, so the walk is broken",
        declaring.len()
    );
    assert!(
        declaring.contains(NOT_IN_THE_DISPATCHER),
        "{NOT_IN_THE_DISPATCHER} is exempted here but no longer holds a selection at all"
    );

    let expected: BTreeSet<String> =
        declaring.iter().filter(|name| *name != NOT_IN_THE_DISPATCHER).cloned().collect();
    assert_eq!(
        asked, expected,
        "`Selection.any-live()` is what Escape asks, and a surface missing from it holds a set \
         nothing can clear"
    );
    assert_eq!(asked, cleared, "a surface Escape asks about and does not clear swallows the press");
}

/// `any-live()` asks ten globals without knowing which surface is on screen, so a set held by the
/// page *under* Now Playing would otherwise swallow the press meant to close it.
#[test]
fn a_selection_press_is_the_last_arm_of_the_escape_chain() {
    let sources = slint_sources();
    let scope = sources
        .iter()
        .find(|(path, _)| path == "layout/shortcut-scope.slint")
        .map_or("", |(_, src)| src.as_str());
    let escape = block_after(scope, "if (ev.text == Key.Escape)");

    let at = |needle: &str| escape.find(needle);
    let selection = at("Selection.any-live()");
    assert!(selection.is_some(), "the Escape chain no longer reaches a live selection at all");

    for earlier in ["Dialog.open", "Onboarding.open", "queue-sheet-up", "Nav.now-playing-open"] {
        let arm = at(earlier);
        assert!(arm.is_some(), "the Escape chain no longer carries an arm for {earlier}");
        assert!(arm < selection, "the selection arm has moved ahead of {earlier}");
    }
}

/// The menu is the right-click path to what a card's hover buttons already do, so an entry it
/// hides while the button beside it stays live reads as a bug on the card being pointed at. It
/// shipped that way once, with `card-is-smart` gating Rename and Edit Artwork as well.
#[test]
fn a_card_menu_entry_is_gated_on_the_operation_and_not_the_kind() {
    const LOOKAHEAD: usize = 6;
    let sources = slint_sources();
    let menu = sources
        .iter()
        .find(|(path, _)| path == "components/grid/card-context-menu.slint")
        .map_or("", |(_, src)| src.as_str());

    let code: Vec<&str> =
        menu.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<&str>>();
    let gates: Vec<usize> = code
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("if ") && line.contains("card-is-smart"))
        .map(|(index, _)| index)
        .collect();

    assert_eq!(
        gates.len(),
        1,
        "only Edit Rules is a smart playlist's alone; every other entry here is offered by the \
         card's own hover trio and by the detail band"
    );
    let reach = gates.first().copied().unwrap_or(0);
    let entry = code.get(reach..(reach + LOOKAHEAD).min(code.len())).unwrap_or(&[]).join(" ");
    assert!(
        entry.contains("root.edit-rules()"),
        "the one kind gate must be Edit Rules', and it now reads: {entry}"
    );
}

/// An animated property cannot land on the frame that dirtied it, so a tint eased beside a snapped
/// ring opens at zero on the frame its partner has already finished. No duration closes that gap;
/// `slint-pitfalls.md` has the mechanism.
#[test]
fn the_card_selection_ring_and_tint_never_animate() {
    let sources = slint_sources();
    let card = sources
        .iter()
        .find(|(path, _)| path == "components/grid/entity-card.slint")
        .map_or("", |(_, src)| src.as_str());

    assert!(card.contains("if root.selected:"), "the selection tint is no longer its own branch");
    let tint = block_after(card, "if root.selected:");
    assert!(
        !tint.contains("animate"),
        "the selection tint must land on the frame the ring does, which no easing can do"
    );
    assert!(
        !card.contains("animate border-width"),
        "the selection ring is a plain `border-width`, for the same reason"
    );
}

/// Admitting a column a short grid cannot fill shrinks every card on screen to make room for
/// nothing, which is what widening the window did to a page holding five playlists. The count has
/// to come off the row model: a published count is a second thing to write, in order, at all ten
/// chunk sites.
#[test]
fn a_lone_row_grid_asks_its_model_how_many_cards_it_drew() {
    let sources = slint_sources();
    let mut mounts: BTreeSet<&str> = BTreeSet::new();

    for (path, src) in &sources {
        if path == "components/grid-geometry.slint" || !src.contains("lone-row-cards") {
            continue;
        }
        mounts.insert(path.as_str());
        let counted = melodia_testkit::binding_value(src, "lone-row-cards");
        for needle in [".length == 1", "[0]", "-1"] {
            assert!(
                counted.contains(needle),
                "{path} derives its lone-row count without {needle}: it reads \
                 `rows.length == 1 ? rows[0].<field>.length : -1` off whichever model the mounted \
                 tab drew, never a count property written beside it"
            );
        }
    }

    assert_eq!(
        mounts,
        LONE_ROW_MOUNTS.into_iter().collect::<BTreeSet<&str>>(),
        "a grid that stops passing `lone-row-cards` goes back to packing a column it cannot fill"
    );
}
