//! Translation-coverage pin: every `@tr(...)` literal in the Slint tree has a
//! `msgid` in every shipped catalogue.
//!
//! It walks the sources rather than pinning a list, so a seventh locale extends
//! the check by appearing in [`SUPPORTED_LOCALES`] and dropping a `.po` beside
//! its siblings — nothing here has to be edited for it.
//!
//! An untranslated string is invisible in review and invisible at runtime in
//! English: Slint falls back to the msgid, so the app renders the source text
//! and looks fine. Eighteen of them had accumulated that way, the whole
//! delete-playlist confirmation among them.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use melodia_core::entities::locale::SUPPORTED_LOCALES;
use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, slint_sources, strip_line_comments, stripped_sources,
};

const TRANSLATIONS_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-ui/translations");

/// The gettext ids one source — a `.slint` file or a `.po` — declares.
#[derive(Default)]
struct Msgids {
    singular: BTreeSet<String>,
    /// `(msgid, msgid_plural)` for the `@tr("one" | "many" % n)` form.
    plural: BTreeSet<(String, String)>,
}

/// Reads the double-quoted literal at or after `from`, skipping leading
/// whitespace (newlines included — three `@tr(` calls in the tree put their
/// literal on the next line). Returns the body still escaped and the index past
/// the closing quote: the catalogues store the same escaping, so the two sides
/// compare without either being unescaped.
fn read_literal(src: &str, from: usize) -> Option<(&str, usize)> {
    let bytes = src.as_bytes();
    let mut i = from;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    let start = i;
    while i < bytes.len() {
        match bytes[i] {
            // Every escape in the tree escapes an ASCII byte, so this can't
            // land mid-codepoint.
            b'\\' => i += 2,
            b'"' => return src.get(start..i).map(|body| (body, i + 1)),
            _ => i += 1,
        }
    }
    None
}

fn collect_from_slint(src: &str, into: &mut Msgids) {
    let stripped = strip_line_comments(src);
    let bytes = stripped.as_bytes();
    let mut at = 0;
    while let Some(offset) = stripped.get(at..).and_then(|rest| rest.find("@tr(")) {
        at += offset + "@tr(".len();
        let Some((one, after)) = read_literal(&stripped, at) else {
            continue;
        };
        into.singular.insert(one.to_owned());

        let mut i = after;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i) == Some(&b'|')
            && let Some((many, _)) = read_literal(&stripped, i + 1)
        {
            into.plural.insert((one.to_owned(), many.to_owned()));
        }
    }
}

/// Reads the value a `msgid` / `msgid_plural` line opens, following gettext's
/// continuation form: an empty literal on the keyword line, then one `"…"` line
/// per chunk. Every entry in these catalogues is on one line today, but
/// `msgfmt` and every PO editor wrap long strings that way — a parser stopping
/// at the keyword line would then read `""` and report the whole tree missing.
/// The chunks concatenate to the same still-escaped body the one-line form
/// gives, so both sides still compare without either being unescaped.
fn read_po_value(lines: &[&str], at: usize, keyword: &str) -> Option<String> {
    let (head, _) = read_literal(lines.get(at)?, keyword.len())?;
    let mut value = head.to_owned();
    for line in lines.get(at + 1..).unwrap_or_default() {
        if !line.starts_with('"') {
            break;
        }
        let Some((chunk, _)) = read_literal(line, 0) else {
            break;
        };
        value.push_str(chunk);
    }
    Some(value)
}

fn collect_from_catalogue(po: &str) -> Msgids {
    let mut ids = Msgids::default();
    let lines: Vec<&str> = po.lines().collect();
    let mut previous: Option<String> = None;
    for (at, line) in lines.iter().enumerate() {
        // `msgid_plural` first — it also starts with `msgid`.
        if line.starts_with("msgid_plural") {
            if let (Some(one), Some(many)) =
                (previous.as_ref(), read_po_value(&lines, at, "msgid_plural"))
            {
                ids.plural.insert((one.clone(), many));
            }
        } else if line.starts_with("msgid")
            && let Some(one) = read_po_value(&lines, at, "msgid")
        {
            ids.singular.insert(one.clone());
            previous = Some(one);
        }
    }
    ids
}

/// Returns every source found, the paths that couldn't be read, and the ids the
/// readable ones declare. Unreadable paths are handed back rather than skipped:
/// silently dropping one lowers the msgid count and the pin goes quiet.
fn slint_tree_msgids() -> (Vec<PathBuf>, Vec<PathBuf>, Msgids) {
    let (sources, mut unreadable) = slint_sources();

    let mut ids = Msgids::default();
    for path in &sources {
        match fs::read_to_string(path) {
            Ok(src) => collect_from_slint(&src, &mut ids),
            Err(_) => unreadable.push(path.clone()),
        }
    }
    (sources, unreadable, ids)
}

/// The pin. Runs the same extraction Slint's codegen does — every `@tr(...)`
/// registers a msgid — and asks each catalogue for it.
#[test]
fn every_translated_literal_has_a_msgid_in_every_catalogue() {
    let (sources, unreadable, wanted) = slint_tree_msgids();
    assert!(unreadable.is_empty(), "unreadable Slint paths: {unreadable:?}");

    // Floors, so a walk that silently found nothing can't pass vacuously. The
    // tree is well past all three; these only catch a broken traversal.
    assert!(
        sources.len() >= MIN_SLINT_SOURCES,
        "only {} .slint files found under {UI_DIR}",
        sources.len()
    );
    assert!(wanted.singular.len() > 400, "only {} msgids extracted", wanted.singular.len());
    assert!(wanted.plural.len() >= 10, "only {} plural pairs extracted", wanted.plural.len());

    for code in SUPPORTED_LOCALES.iter().filter(|code| **code != "en") {
        let path = PathBuf::from(TRANSLATIONS_DIR).join(code).join("LC_MESSAGES/melodia-ui.po");
        let catalogue = fs::read_to_string(&path);
        assert!(
            catalogue.is_ok(),
            "{code} is in SUPPORTED_LOCALES but has no catalogue at {}",
            path.display()
        );
        let Ok(po) = catalogue else { continue };
        let have = collect_from_catalogue(&po);

        let missing: Vec<&str> =
            wanted.singular.difference(&have.singular).map(String::as_str).collect();
        assert!(missing.is_empty(), "{code} is missing {} msgid(s): {missing:?}", missing.len());

        let missing_plurals: Vec<&(String, String)> =
            wanted.plural.difference(&have.plural).collect();
        assert!(
            missing_plurals.is_empty(),
            "{code} is missing {} msgid_plural pair(s): {missing_plurals:?}",
            missing_plurals.len()
        );
    }
}

/// English is the source baseline and ships no catalogue; every other supported
/// code must have one, or `select_bundled_translation` silently leaves the UI in
/// English for that pick.
#[test]
fn every_supported_locale_but_english_ships_a_catalogue() {
    for code in SUPPORTED_LOCALES {
        let path = PathBuf::from(TRANSLATIONS_DIR).join(code).join("LC_MESSAGES/melodia-ui.po");
        assert_eq!(
            path.is_file(),
            *code != "en",
            "catalogue presence doesn't match the locale list for {code} ({})",
            path.display()
        );
    }
}

/// Every `"Unknown …"` field fallback goes through `@tr`.
///
/// The class the pin above structurally cannot see: an unwrapped literal declares no msgid,
/// so a catalogue that never hears of it is not a gap a msgid walk can find. Both track-list
/// cells shipped as bare `"Unknown Artist"` / `"Unknown Album"` while the grid card and the
/// now-playing line beside them already said `@tr("Unknown artist")` — English on all six
/// locales, and invisible to a reviewer reading either site on its own.
///
/// Scoped to this one phrase rather than "every literal": most string literals in the tree
/// are icon ligatures, theme tokens, asset paths and view-context tags that must *not* be
/// translated, so a general rule would be a list of exemptions wearing a walk's clothes.
#[test]
fn every_unknown_field_fallback_is_translated() {
    const FALLBACK: &str = "\"Unknown ";

    let mut offenders: Vec<String> = Vec::new();
    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        let mut at = 0;
        while let Some(offset) = src.get(at..).and_then(|rest| rest.find(FALLBACK)) {
            let quote = at + offset;
            at = quote + FALLBACK.len();
            // `trim_end` rather than an exact prefix: three `@tr(` calls in the tree put
            // their literal on the next line.
            if src.get(..quote).is_some_and(|head| head.trim_end().ends_with("@tr(")) {
                continue;
            }
            let shown = read_literal(&src, quote).map_or_else(String::new, |(body, _)| body.into());
            offenders.push(format!("{path}: \"{shown}\""));
        }
    }

    assert!(
        offenders.is_empty(),
        "{offenders:?} paint an untranslated fallback — wrap it in `@tr(…)` and add the msgid \
         to all six catalogues. Reuse `Unknown artist` / `Unknown album` verbatim rather than \
         title-casing a second entry that says the same thing."
    );
}
