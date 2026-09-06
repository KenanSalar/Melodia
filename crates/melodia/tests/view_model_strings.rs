//! The values that cross into Slint as **free strings**, and the `.slint` literals that branch
//! on them.
//!
//! Three of them: `RepeatMode` and `PlaybackStatus` reach `PlayerViewModel` through
//! `ui::shell::bridge` as whatever `as_str` returns, and a notification's `variant` reaches
//! `NotificationCard` as whatever the producer typed. Nothing on either side of any of the three
//! is checked by a compiler, and none of the failures is loud: a renamed `RepeatMode::Off` leaves
//! the repeat button lit in every state, because the sheet asks `!= "off"`; an unknown `variant`
//! falls through to info styling, which is a real arm rather than a crash.
//!
//! Here rather than beside either tree, and this is the case `CLAUDE.md` draws the line for: the
//! claim is about two trees at once, so neither the enum's own suite nor a `.slint` pin can hold
//! it. The enums' Rust halves stay where they are, in `engine/tests/types_tests.rs`.
//!
//! The direction is deliberate for the two enums. **Every literal the sheet compares against must
//! be one the enum can produce**, not the reverse: `RepeatMode::All` is legitimately never named
//! in a comparison, the sheet distinguishing only "one" from "not off". A rename on *either* side
//! still fails it, since renaming the Rust variant is what makes the sheet's literal unproducible.
//! The `variant` half is an equality, there being no such asymmetry to allow for.

use std::collections::BTreeSet;

use melodia_engine::player::engine::types::{PlaybackStatus, RepeatMode};
use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, rust_sources, stripped_sources};

/// The card, by the path `stripped_sources` hands back.
const CARD: &str = "components/dialog/notification-stack.slint";

/// The fourth variant, which the card names as a fallback icon rather than dispatching on.
/// Pinned separately below so this constant is not a free-floating claim about the sheet.
const FALLBACK_VARIANT: &str = "info";

/// Vacuity floors, one per walk. Each is under what the tree holds today and none is an
/// inventory: what they guard is a walk that stopped matching, which every assertion below
/// would otherwise pass. Deliberately loose, `status` most of all — its two comparisons are the
/// two halves of one ternary, so anything but 1 is a floor that fails on an ordinary edit.
const MIN_REPEAT_SITES: usize = 6;
const MIN_STATUS_SITES: usize = 1;
const MIN_VARIANT_PRODUCERS: usize = 15;

/// The literal `src` compares `.field` against at `at`, where `at` is the offset of the `.`.
///
/// `None` when the comparison is against anything but a string, which is every read that is not
/// one of these contracts. The field name must end at a non-identifier byte, so `.variant` does
/// not match a longer name ending in it.
fn compared_literal<'a>(src: &'a str, at: usize, field: &str) -> Option<&'a str> {
    let bytes = src.as_bytes();
    let mut i = at + 1 + field.len();
    if bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-') {
        return None;
    }
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if !matches!(src.get(i..i + 2), Some("==" | "!=")) {
        return None;
    }
    i += 2;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    let open = i + 1;
    src[open..].find('"').map(|end| &src[open..open + end])
}

/// Every string literal `src` compares `.field` against, in source order.
fn compared_literals_in<'a>(src: &'a str, field: &str) -> Vec<&'a str> {
    let needle = format!(".{field}");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find(&needle).map(|rel| rel + from) {
        from = at + needle.len();
        if let Some(literal) = compared_literal(src, at, field) {
            out.push(literal);
        }
    }
    out
}

/// [`compared_literals_in`] over the whole Slint tree, paired with the file each came from.
fn compared_literals(field: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        for literal in compared_literals_in(&src, field) {
            out.push((path.clone(), literal.to_owned()));
        }
    }
    out
}

/// The whole of one `private property <…> name:` binding, up to its terminating `;`.
fn binding_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let at = src.find(&format!("{name}:"))? + name.len() + 1;
    src[at..].find(';').map(|end| &src[at..at + end])
}

/// The card's source, comment-stripped.
fn card_source() -> String {
    stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .find(|(path, _)| path == CARD)
        .map(|(_, src)| src)
        .unwrap_or_default()
}

/// The string literal at `at`, if that is where one opens.
fn literal_at(src: &str, at: usize) -> Option<&str> {
    let mut i = at;
    while src.as_bytes().get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if src.as_bytes().get(i) != Some(&b'"') {
        return None;
    }
    let open = i + 1;
    src[open..].find('"').map(|end| &src[open..open + end])
}

/// The literal `src` passes as the argument `commas` positions into the call opened at `from`,
/// which is the offset of the `(`.
///
/// `None` where that argument is an identifier rather than a literal, which is the shape
/// [`variant_literals_from_bindings`] picks up instead. Depth-aware, so a closure or a nested
/// call in an earlier argument does not shift the count.
fn arg_literal(src: &str, from: usize, commas: usize) -> Option<&str> {
    if commas == 0 {
        return literal_at(src, from + 1);
    }
    let bytes = src.as_bytes();
    let (mut depth, mut seen, mut i) = (0usize, 0usize, from);
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return None;
                }
            }
            b',' if depth == 1 => {
                seen += 1;
                if seen == commas {
                    return literal_at(src, i + 1);
                }
            }
            b'"' => {
                let close = src[i + 1..].find('"')?;
                i += close + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Every literal in `src` following `anchor`, one per occurrence, at argument `commas`.
fn call_arg_literals<'a>(src: &'a str, anchor: &str, commas: usize) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find(anchor).map(|rel| rel + from) {
        let open = at + anchor.len() - 1;
        from = at + anchor.len();
        if let Some(literal) = arg_literal(src, open, commas) {
            out.push(literal);
        }
    }
    out
}

/// The literals a `let variant = …;` statement can settle on.
///
/// Five `NotificationParams::plain` call sites take the variant as an identifier bound just
/// above, so the call itself carries no literal to read. Its caller scopes this to files naming
/// that constructor, which is what keeps `ui::appearance`'s theme-variant bindings out: those are
/// the same statement about a different `variant`.
fn variant_literals_from_bindings(src: &str) -> Vec<&str> {
    const BINDING: &str = "let variant = ";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find(BINDING).map(|rel| rel + from) {
        from = at + BINDING.len();
        match src[from..].find(';') {
            Some(end) => out.extend(quoted_literals(&src[from..from + end])),
            None => break,
        }
    }
    out
}

/// Every double-quoted literal in `span`, in source order.
fn quoted_literals(span: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = span;
    while let Some(open) = rest.find('"') {
        let Some(close) = rest[open + 1..].find('"') else {
            break;
        };
        out.push(&rest[open + 1..open + 1 + close]);
        rest = &rest[open + 1 + close + 1..];
    }
    out
}

/// Every notification variant the Rust tree can hand the card, paired with the file it came from.
///
/// Three shapes, and all three are in the tree today: `show_localized`'s second argument,
/// a `NotificationParams` struct literal's `variant` field, and the `let variant` ternary the
/// `plain` constructor's callers bind above the call.
fn rust_variant_literals() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (path, src) in rust_sources() {
        if !src.contains("NotificationParams") && !src.contains("show_localized") {
            continue;
        }
        let mut found: Vec<&str> = call_arg_literals(&src, "show_localized(", 1);
        found.extend(call_arg_literals(&src, "NotificationParams::plain(", 0));
        if src.contains("NotificationParams::plain(") {
            found.extend(variant_literals_from_bindings(&src));
        }
        let mut from = 0;
        while let Some(at) = src[from..].find("variant: \"").map(|rel| rel + from) {
            let open = at + "variant: \"".len();
            from = open;
            if let Some(end) = src[open..].find('"') {
                found.push(&src[open..open + end]);
            }
        }
        out.extend(found.into_iter().map(|literal| (path.clone(), literal.to_owned())));
    }
    out
}

#[test]
fn every_repeat_mode_the_sheet_branches_on_is_one_the_enum_produces() {
    let produced: BTreeSet<&str> = [RepeatMode::Off, RepeatMode::All, RepeatMode::One]
        .iter()
        .map(RepeatMode::as_str)
        .collect();

    let sites = compared_literals("repeat_mode");
    assert!(
        sites.len() >= MIN_REPEAT_SITES,
        "only {} `repeat_mode` comparisons found under {UI_DIR}; the walk has stopped matching \
         and every assertion standing on it now passes vacuously",
        sites.len()
    );

    for (path, literal) in &sites {
        assert!(
            produced.contains(literal.as_str()),
            "{path} branches on `repeat_mode == \"{literal}\"`, which no `RepeatMode` variant \
             produces (it produces {produced:?}). The branch is dead: the repeat button paints \
             one state for every mode, with nothing failing to say so"
        );
    }
}

#[test]
fn every_playback_status_the_sheet_branches_on_is_one_the_enum_produces() {
    let produced: BTreeSet<&str> = [
        PlaybackStatus::Stopped,
        PlaybackStatus::Playing,
        PlaybackStatus::Paused,
        PlaybackStatus::Loading,
    ]
    .iter()
    .map(PlaybackStatus::as_str)
    .collect();

    let sites = compared_literals("status");
    assert!(
        sites.len() >= MIN_STATUS_SITES,
        "only {} `status` comparisons found under {UI_DIR}; the walk has stopped matching",
        sites.len()
    );

    for (path, literal) in &sites {
        assert!(
            produced.contains(literal.as_str()),
            "{path} branches on `status == \"{literal}\"`, which no `PlaybackStatus` variant \
             produces (it produces {produced:?})"
        );
    }
}

/// A variant carrying a colour but no glyph, or the reverse, is a half-styled card. The two
/// ladders are written one after the other and there is nothing but proximity holding them
/// together.
#[test]
fn the_notification_cards_two_ladders_style_the_same_variants() {
    let src = card_source();
    let brush = binding_body(&src, "accent-brush");
    let icon = binding_body(&src, "variant-icon");
    assert!(brush.is_some() && icon.is_some(), "{CARD}: one of the two variant ladders is gone");
    let (Some(brush), Some(icon)) = (brush, icon) else {
        return;
    };

    let styled: BTreeSet<&str> = compared_literals_in(brush, "variant").into_iter().collect();
    let glyphed: BTreeSet<&str> = compared_literals_in(icon, "variant").into_iter().collect();

    assert!(!styled.is_empty(), "{CARD}: the accent-brush ladder dispatches on nothing");
    assert_eq!(
        styled, glyphed,
        "{CARD}: the accent-brush and variant-icon ladders dispatch on different variants, so \
         one of them paints a card the other leaves at the fallback"
    );
    assert!(
        icon.contains(&format!("\"{FALLBACK_VARIANT}\"")),
        "{CARD}: the variant-icon ladder no longer falls back to \"{FALLBACK_VARIANT}\", which \
         is the fourth variant Rust sends and the one this file's ladders never name"
    );
}

/// An equality rather than a containment, unlike the two enums above: the producers are string
/// literals with no enum behind them, so a variant Rust sends that the card does not style is
/// silently info-coloured, and an arm the card carries that nothing sends is dead sheet.
#[test]
fn the_variants_rust_sends_are_exactly_the_ones_the_card_styles() {
    let card = card_source();
    let icon = binding_body(&card, "variant-icon");
    assert!(icon.is_some(), "{CARD}: the variant-icon ladder is gone");
    let Some(icon) = icon else { return };

    let mut styled: BTreeSet<&str> = compared_literals_in(icon, "variant").into_iter().collect();
    styled.insert(FALLBACK_VARIANT);

    let producers = rust_variant_literals();
    assert!(
        producers.len() >= MIN_VARIANT_PRODUCERS,
        "only {} notification-variant literals found in the Rust tree; the walk has stopped \
         matching one of its three shapes",
        producers.len()
    );

    let sent: BTreeSet<&str> = producers.iter().map(|(_, literal)| literal.as_str()).collect();
    assert_eq!(
        sent, styled,
        "the notification variants Rust sends and the ones {CARD} styles have drifted. A sent \
         variant the card does not know is painted as \"{FALLBACK_VARIANT}\" with nothing \
         reporting it; a styled one nothing sends is an arm no user can reach. Producers: \
         {producers:?}"
    );
}
