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
    MIN_SLINT_SOURCES, UI_DIR, binding_value, block_after, blocks_named, outside_strings,
    stripped_sources,
};

/// The button every right-click menu answers to.
const RIGHT_BUTTON: &str = "PointerEventButton.right";

/// The predicate's file and how often it names the button. An exact count rather than a bare
/// exemption: a second spelling there is a second rule beside the predicate, and none at all
/// means the needle has gone stale and this walk reads nothing.
const SANCTIONED: [(&str, usize); 1] = [("components/context-menu.slint", 1)];

/// The popup an item menu opens, which is what the walk finds the menus by.
const ITEM_MENU_POPUP: &str = "ContextMenuPopup";

/// The menus a row or card raises. An equality, so a menu that stops opening the popup can't
/// leave the walk unnoticed, nor a renamed popup empty it and pass.
const ITEM_MENUS: [&str; 3] = ["CardContextMenu", "StationContextMenu", "TrackContextMenu"];

/// The callback a card raises its menu through, the menu being its host's.
const MENU_REQUEST: &str = "callback context-requested(";

/// The cards that raise one, held for `ITEM_MENUS`' reason.
const MENU_REQUESTERS: [&str; 2] = ["EntityCard", "StationCard"];

/// The binding a card is told its menu is open through.
const MENU_OPEN: &str = "menu-open:";

/// The raw hover flag, read once per item: where its menu is folded in.
const HAS_HOVER: &str = ".has-hover";

/// `EntityCard`'s own `TouchArea`, whose raw hover only `card-hovered` may read.
const CARD_TOUCH: &str = "touch";

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

/// The statements in `src` lighting an item off `{id}.is-open`: a card host's `menu-open`
/// binding, or a row's OR with its raw hover.
fn lighting<'a>(src: &'a str, id: &str) -> Vec<&'a str> {
    let needle = format!("{id}.is-open");
    src.match_indices(&needle)
        .filter_map(|(at, _)| {
            let start = src[..at].rfind([';', '{', '}']).map_or(0, |open| open + 1);
            let end = at + src[at..].find(';')?;
            Some(src[start..end].trim())
        })
        .filter(|statement| statement.contains(MENU_OPEN) || !hover_areas(statement).is_empty())
        .collect()
}

/// The `TouchArea` ids whose raw hover `statement` reads.
fn hover_areas(statement: &str) -> Vec<&str> {
    statement
        .match_indices(HAS_HOVER)
        .map(|(at, _)| {
            let head = &statement[..at];
            &head[head.trim_end_matches(is_id_char).len()..]
        })
        .collect()
}

/// How often `src` reads `area`'s raw hover, a longer id ending in `area` not counting.
fn hover_reads(src: &str, area: &str) -> usize {
    let needle = format!("{area}{HAS_HOVER}");
    src.match_indices(&needle)
        .filter(|(at, _)| src[..*at].chars().next_back().is_none_or(|ch| !is_id_char(ch)))
        .count()
}

/// Whether `ch` can sit inside a Slint id.
fn is_id_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'
}

/// `body` without its nested blocks, so a binding on a child can't answer for the mount around
/// it. Quote-aware for `block_body`'s reason: the `{n}` in an `@tr` opens no block.
fn top_level(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut depth = 0usize;
    let mut kept_from = 0;
    for (at, byte) in outside_strings(body.as_bytes()) {
        match byte {
            b'{' => {
                if depth == 0 {
                    out.push_str(&body[kept_from..at]);
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    kept_from = at + 1;
                }
            }
            _ => {}
        }
    }
    if depth == 0 {
        out.push_str(&body[kept_from..]);
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

/// An item lit by its own `has-hover` goes dark under the menu it raised, an open popup taking the
/// hover, and nothing marks what the menu is for. A host folds `is-open` into the one binding
/// reading its raw hover, so a second raw read is a hover binding that went around it. Neither miss
/// shows until a menu is open.
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
    let mut unreadable = Vec::new();
    let mut dark = Vec::new();
    let mut bypassed = Vec::new();
    for (path, src) in &sources {
        for menu in &names {
            let ids = mount_ids(src, menu);
            if ids.len() != blocks_named(src, menu).len() {
                unreadable.push(format!("{path}: {menu}"));
            }
            for id in ids {
                mounted.insert(*menu);
                let lit = lighting(src, id);
                if lit.is_empty() {
                    dark.push(format!("{path}: {id}"));
                }
                bypassed.extend(
                    lit.iter()
                        .flat_map(|statement| hover_areas(statement))
                        .filter(|area| hover_reads(src, area) > 1)
                        .map(|area| format!("{path}: {area}")),
                );
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
        unreadable.is_empty(),
        "these files mount a menu in a shape the walk can't read, so its host goes unchecked: \
         {unreadable:?}"
    );
    assert!(
        dark.is_empty(),
        "these hosts read only their own hover, so they go dark under their own menu: {dark:?}"
    );
    assert!(
        bypassed.is_empty(),
        "these hover areas are read raw outside the binding their menu is folded into, so what \
         reads them goes dark under the menu: {bypassed:?}"
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
                let told = binding_value(&own, MENU_OPEN).trim();
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
    assert_eq!(
        hover_reads(card, CARD_TOUCH),
        1,
        "a card binding reading its raw hover instead of `card-hovered` goes dark under the menu"
    );
}
