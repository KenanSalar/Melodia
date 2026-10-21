//! What a right-click menu owes the item it was raised on: it opens on the button's release,
//! through one predicate, and it keeps that item lit for as long as it is up.
//!
//! Slint closes an open popup on a press outside it and swallows that press, so a menu raised on
//! the press can't be moved by a right-click elsewhere: the press only closes the old menu, and the
//! user clicks twice. A menu raised on the release answers that same right-click with its own
//! release. A press-raised menu still opens on the first click and nothing about it looks wrong in
//! review, so the button itself is the needle: `ContextClick.raises-menu` is the one place that may
//! name it.

use std::collections::BTreeSet;

use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, binding_value, block_after, blocks_named, stripped_sources,
};

/// The button every right-click menu answers to.
const RIGHT_BUTTON: &str = "PointerEventButton.right";

/// The predicate's file and how often it names the button. An exact count rather than a bare
/// exemption: a second spelling there is a second rule beside the predicate, and none at all
/// means the needle has gone stale and this walk reads nothing.
const SANCTIONED: [(&str, usize); 1] = [("components/context-menu.slint", 1)];

/// The popup an item menu opens, which is what the walk finds the menus by.
const ITEM_MENU_POPUP: &str = "ContextMenuPopup";

/// The menus a row or card raises. An equality, so a fourth menu has to be looked at rather than
/// joining the walk unread, and a renamed popup can't empty the set and pass.
const ITEM_MENUS: [&str; 3] = ["CardContextMenu", "StationContextMenu", "TrackContextMenu"];

/// The callback a card raises its menu through, the menu being its host's.
const MENU_REQUEST: &str = "callback context-requested(";

/// The cards that raise one, held for `ITEM_MENUS`' reason.
const MENU_REQUESTERS: [&str; 2] = ["EntityCard", "StationCard"];

/// Every `component` declared in `src`, as its name and body.
fn components(src: &str) -> Vec<(&str, &str)> {
    src.match_indices("component ")
        .filter(|(at, _)| src[..*at].chars().next_back().is_none_or(char::is_whitespace))
        .filter_map(|(at, keyword)| {
            let rest = &src[at + keyword.len()..];
            let name = rest.split_whitespace().next()?;
            Some((name, block_after(rest, name)))
        })
        .collect()
}

/// The id of every `<id> := <component> {` in `src`. A menu mounted without one could never be
/// shown, so every mount the walk needs has one.
fn mount_ids<'a>(src: &'a str, component: &str) -> Vec<&'a str> {
    let needle = format!(":= {component} {{");
    src.match_indices(&needle)
        .filter_map(|(at, _)| src[..at].split_whitespace().next_back())
        .collect()
}

/// `body` without its nested blocks, so a binding on a child can't answer for the mount around
/// it. Quote-aware for `block_body`'s reason: the `{n}` in an `@tr` opens no block.
fn top_level(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for ch in body.chars() {
        let outer = depth == 0;
        match ch {
            '"' if !escaped => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => depth = depth.saturating_sub(1),
            _ => {}
        }
        escaped = in_string && ch == '\\' && !escaped;
        if outer && depth == 0 {
            out.push(ch);
        }
    }
    out
}

#[test]
fn only_the_shared_predicate_names_the_right_button() {
    let spelled: Vec<(String, usize)> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .map(|(path, src)| {
            let count = src.matches(RIGHT_BUTTON).count();
            (path, count)
        })
        .filter(|(_, count)| *count > 0)
        .collect();
    let sanctioned: Vec<(String, usize)> =
        SANCTIONED.iter().map(|(path, count)| ((*path).to_owned(), *count)).collect();

    assert_eq!(
        spelled, sanctioned,
        "a surface testing the right button itself opens its menu on the press, which a \
         right-click elsewhere can't move; ask `ContextClick.raises-menu` instead"
    );
}

/// An open popup takes every pointer event and sends the rest of the window an exit, so a row
/// lit by its own `has-hover` goes dark under the menu it raised and nothing marks which item the
/// menu is for. The menu says it is open and the host reads that beside its hover; a host that
/// doesn't still opens its menu and looks right until it does.
#[test]
fn every_item_menu_host_reads_whether_its_menu_is_open() {
    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    let menus: Vec<(&str, &str)> = sources
        .iter()
        .flat_map(|(_, src)| components(src))
        .filter(|(_, body)| !mount_ids(body, ITEM_MENU_POPUP).is_empty())
        .collect();
    let names: BTreeSet<&str> = menus.iter().map(|(name, _)| *name).collect();

    let mut unpublished = Vec::new();
    for (name, body) in &menus {
        let published = binding_value(body, "out property <bool> is-open:").trim();
        let popups = mount_ids(body, ITEM_MENU_POPUP);
        if !popups.iter().any(|popup| published == format!("{popup}.is-open")) {
            unpublished.push(*name);
        }
    }

    let mut mounted = BTreeSet::new();
    let mut dark = Vec::new();
    for (path, src) in &sources {
        for menu in &names {
            for id in mount_ids(src, menu) {
                mounted.insert(*menu);
                if !src.contains(&format!("{id}.is-open")) {
                    dark.push(format!("{path}: {id}"));
                }
            }
        }
    }

    assert_eq!(
        names,
        ITEM_MENUS.into_iter().collect::<BTreeSet<&str>>(),
        "the item menus are found by the popup they open; a new one owes its host the same light"
    );
    assert!(
        unpublished.is_empty(),
        "an item menu's `is-open` has to be its popup's, or every host reading it stays dark: \
         {unpublished:?}"
    );
    assert_eq!(
        mounted, names,
        "a menu with no mount the walk can see means its needle has gone stale"
    );
    assert!(
        dark.is_empty(),
        "these hosts read only their own hover, so they go dark under their own menu: {dark:?}"
    );
}

/// A card's menu is its host's, so the card can't see it open and has to be told. Two links can
/// drop out with everything still building: a mount that raises the menu without passing its
/// state on, and the card no longer counting that state as hover.
#[test]
fn every_card_raising_a_menu_is_told_while_it_is_open() {
    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    let requesters: BTreeSet<&str> = sources
        .iter()
        .flat_map(|(_, src)| components(src))
        .filter(|(_, body)| body.contains(MENU_REQUEST))
        .map(|(name, _)| name)
        .collect();

    let mut raising = BTreeSet::new();
    let mut untold = Vec::new();
    for (path, src) in &sources {
        for requester in &requesters {
            for body in blocks_named(src, requester) {
                let own = top_level(body);
                if !own.contains("context-requested(") {
                    continue;
                }
                raising.insert(*requester);
                let told = binding_value(&own, "menu-open:").trim();
                if !(told.ends_with(".menu-open") || told.ends_with(".is-open")) {
                    untold.push(format!("{path}: {requester}"));
                }
            }
        }
    }

    let card = sources
        .iter()
        .find(|(path, _)| path == "components/grid/entity-card.slint")
        .map_or("", |(_, src)| src.as_str());
    let hovered = binding_value(card, "out property <bool> card-hovered:");

    assert_eq!(
        requesters,
        MENU_REQUESTERS.into_iter().collect::<BTreeSet<&str>>(),
        "a card raising its own menu owes its mounts the same `menu-open` binding"
    );
    assert_eq!(
        raising, requesters,
        "a card nothing raises a menu from means the mount walk has stopped reading"
    );
    assert!(
        untold.is_empty(),
        "these mounts raise a menu without saying when it is open, so the card goes dark under \
         it: {untold:?}"
    );
    assert!(
        hovered.contains("root.menu-open"),
        "`card-hovered` is what the fill, zoom and controls read, so an open menu has to count"
    );
}
