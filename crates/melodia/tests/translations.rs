//! Pins the translation catalogues to the Slint tree `scripts/update-translations.sh` generates
//! them from.
//!
//! It walks the sources rather than pinning a list, so a seventh locale extends
//! the check by appearing in [`SUPPORTED_LOCALES`] and dropping a `.po` beside
//! its siblings — nothing here has to be edited for it.
//!
//! An untranslated string is invisible in review and invisible at runtime in
//! English: Slint falls back to the msgid, so the app renders the source text
//! and looks fine. Eighteen of them had accumulated that way, the whole
//! delete-playlist confirmation among them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs;
use std::iter;
use std::path::{Path, PathBuf};

use melodia_core::entities::locale::SUPPORTED_LOCALES;
use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, raw_sources, strip_line_comments, stripped_sources,
};

const TRANSLATIONS_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-ui/translations");

/// Floor under every msgid count here, the tree's and each parsed file's, so a broken walk or
/// parser can't pass vacuously. Loose on purpose; it only catches a traversal or a parse that
/// found next to nothing.
const MIN_MSGIDS: usize = 400;

/// Floor under the plural pairs found in the tree, loose for [`MIN_MSGIDS`]' reason.
const MIN_PLURALS: usize = 10;

/// Floor under the translator notes found in the tree, loose for [`MIN_MSGIDS`]' reason.
const MIN_NOTES: usize = 20;

const RUN_THE_SCRIPT: &str = "run scripts/update-translations.sh";

/// msgid → `msgid_plural`, the plural being there for the `@tr("one" | "many" % n)` form.
type Msgids = BTreeMap<String, Option<String>>;

/// msgid → its translator notes. A set because msgcat folds an identical note repeated across
/// files into one.
type Notes = BTreeMap<String, BTreeSet<String>>;

/// One entry of the template or a catalogue, its strings still escaped as the file spells them.
#[derive(Default)]
struct Entry {
    /// The `#.` lines, which the script lets through only as translator notes.
    notes: Vec<String>,
    fuzzy: bool,
    msgid: String,
    msgid_plural: Option<String>,
    /// The one `msgstr`, or a plural's `msgstr[n]` in order.
    msgstrs: Vec<String>,
}

impl Entry {
    /// Whether Slint passes over this entry for its msgid: it loads only what rspolib's
    /// `translated()` accepts.
    fn shows_in_english(&self) -> bool {
        self.fuzzy || self.msgstrs.is_empty() || self.msgstrs.iter().any(String::is_empty)
    }
}

/// Reads the double-quoted literal at or after `from`, skipping leading
/// whitespace (newlines included, since a `@tr(` can put its literal on the
/// next line). Returns the body still escaped and the index past
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

/// Collects the msgids a comment-stripped `.slint` source registers.
fn collect_from_slint(src: &str, into: &mut Msgids) {
    let bytes = src.as_bytes();
    let mut at = 0;
    while let Some(offset) = src.get(at..).and_then(|rest| rest.find("@tr(")) {
        at += offset + "@tr(".len();
        let Some((one, after)) = read_literal(src, at) else {
            continue;
        };
        let plural = into.entry(one.to_owned()).or_default();

        let mut i = after;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i) == Some(&b'|')
            && let Some((many, _)) = read_literal(src, i + 1)
        {
            *plural = Some(many.to_owned());
        }
    }
}

/// Reads the value a keyword line opens, following gettext's continuation form: an empty
/// literal on the keyword line, then one `"…"` line per chunk. The script writes `--no-wrap`,
/// but every PO editor wraps long strings that way, and a parser stopping at the keyword line
/// would read `""` for a string that is there. The chunks concatenate to the same still-escaped
/// body the one-line form gives, so both sides still compare without either being unescaped.
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

/// Every entry in `po` but the header, passing over obsolete `#~` blocks as Slint does.
fn parse_po(po: &str) -> Vec<Entry> {
    po.split("\n\n").filter_map(parse_entry).collect()
}

fn parse_entry(block: &str) -> Option<Entry> {
    let lines: Vec<&str> = block.lines().collect();
    let mut entry = Entry::default();
    for (at, line) in lines.iter().enumerate() {
        if line.starts_with("#~") {
            return None;
        }
        if let Some(note) = line.strip_prefix("#.") {
            entry.notes.push(note.trim().to_owned());
        } else if let Some(flags) = line.strip_prefix("#,") {
            entry.fuzzy |= flags.split(',').any(|flag| flag.trim() == "fuzzy");
        } else if let Some((keyword, _)) = line.split_once(' ') {
            match keyword {
                "msgid" => entry.msgid = read_po_value(&lines, at, keyword)?,
                "msgid_plural" => entry.msgid_plural = read_po_value(&lines, at, keyword),
                _ if keyword.starts_with("msgstr") => {
                    entry.msgstrs.push(read_po_value(&lines, at, keyword)?);
                }
                _ => {}
            }
        }
    }
    (!entry.msgid.is_empty()).then_some(entry)
}

fn msgids(entries: &[Entry]) -> Msgids {
    entries.iter().map(|entry| (entry.msgid.clone(), entry.msgid_plural.clone())).collect()
}

fn template_notes(entries: &[Entry]) -> Notes {
    entries
        .iter()
        .filter(|entry| !entry.notes.is_empty())
        .map(|entry| (entry.msgid.clone(), entry.notes.iter().cloned().collect()))
        .collect()
}

/// What `left` holds that `right` doesn't hold the same way, for a failure message.
fn unmatched<V: PartialEq + Debug>(
    left: &BTreeMap<String, V>,
    right: &BTreeMap<String, V>,
) -> Vec<String> {
    left.iter()
        .filter(|(key, value)| right.get(*key) != Some(*value))
        .map(|(key, value)| format!("\"{key}\" {value:?}"))
        .collect()
}

/// `path` parsed, asserting it read and held at least [`MIN_MSGIDS`] entries, so a parser that
/// broke can't make every check over it pass on nothing.
fn read_entries(path: &Path) -> Vec<Entry> {
    let po = fs::read_to_string(path);
    assert!(po.is_ok(), "can't read {}: {po:?}", path.display());
    let entries = parse_po(&po.unwrap_or_default());
    assert!(
        entries.len() >= MIN_MSGIDS,
        "only {} entries parsed from {}",
        entries.len(),
        path.display()
    );
    entries
}

fn catalogue_path(code: &str) -> PathBuf {
    PathBuf::from(TRANSLATIONS_DIR).join(code).join("LC_MESSAGES").join("melodia-ui.po")
}

fn template_entries() -> Vec<Entry> {
    read_entries(&PathBuf::from(TRANSLATIONS_DIR).join("melodia-ui.pot"))
}

/// Every shipped catalogue, parsed and paired with its locale code.
fn catalogues() -> Vec<(&'static str, Vec<Entry>)> {
    SUPPORTED_LOCALES
        .iter()
        .filter(|code| **code != "en")
        .map(|code| (*code, read_entries(&catalogue_path(code))))
        .collect()
}

/// The msgids the Slint tree registers, by the same extraction Slint's codegen runs.
fn slint_tree_msgids() -> Msgids {
    let mut ids = Msgids::new();
    for (_, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        collect_from_slint(&src, &mut ids);
    }
    assert!(ids.len() >= MIN_MSGIDS, "only {} msgids extracted from {UI_DIR}", ids.len());
    let plurals = ids.values().filter(|plural| plural.is_some()).count();
    assert!(plurals >= MIN_PLURALS, "only {plurals} plural pairs extracted from {UI_DIR}");
    ids
}

/// The note a `// Translators: …` line carries, spelled as the template's `#.` line spells it.
fn translator_note(line: &str) -> Option<&str> {
    let comment = line.trim_start().strip_prefix("//")?.trim();
    comment.starts_with("Translators:").then_some(comment)
}

/// The msgid of the `@tr(` on `line`, which starts at `start` in `src`, or `None` unless the
/// line holds exactly one: the extractor hands the comment above to every call on the line.
fn sole_tr_call<'a>(src: &'a str, start: usize, line: &str) -> Option<&'a str> {
    let code = strip_line_comments(line);
    let mut calls = code.match_indices("@tr(");
    let (at, _) = calls.next()?;
    if calls.next().is_some() {
        return None;
    }
    read_literal(src, start + at + "@tr(".len()).map(|(msgid, _)| msgid)
}

/// Every translator note in the Slint tree by the msgid below it, and as `path:line` the notes
/// with no single `@tr(` directly below them.
fn slint_tree_notes() -> (Notes, Vec<String>) {
    let mut notes = Notes::new();
    let mut misplaced = Vec::new();
    for (path, src) in raw_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        let starts = iter::once(0).chain(src.match_indices('\n').map(|(at, _)| at + 1));
        let lines: Vec<(usize, &str)> = starts.zip(src.lines()).collect();
        for (index, (_, line)) in lines.iter().enumerate() {
            let Some(note) = translator_note(line) else {
                continue;
            };
            let below = lines.get(index + 1);
            match below.and_then(|&(start, next)| sole_tr_call(&src, start, next)) {
                Some(msgid) => {
                    notes.entry(msgid.to_owned()).or_default().insert(note.to_owned());
                }
                None => misplaced.push(format!("{path}:{}: {note}", index + 1)),
            }
        }
    }
    (notes, misplaced)
}

#[test]
fn the_template_holds_exactly_the_slint_trees_strings() {
    let tree = slint_tree_msgids();
    let template = msgids(&template_entries());

    let missing = unmatched(&tree, &template);
    let stale = unmatched(&template, &tree);
    assert!(
        missing.is_empty() && stale.is_empty(),
        "melodia-ui.pot is out of step with the .slint tree, so {RUN_THE_SCRIPT}. \
         Missing: {missing:?}. Gone from the tree: {stale:?}"
    );
}

#[test]
fn every_catalogue_holds_exactly_the_templates_strings() {
    let template = msgids(&template_entries());

    let mut out_of_step = Vec::new();
    for (code, entries) in catalogues() {
        let catalogue = msgids(&entries);
        let lacks = unmatched(&template, &catalogue);
        let extra = unmatched(&catalogue, &template);
        out_of_step.extend(lacks.into_iter().map(|id| format!("{code} lacks {id}")));
        out_of_step.extend(extra.into_iter().map(|id| format!("{code} has an extra {id}")));
    }
    assert!(
        out_of_step.is_empty(),
        "catalogues out of step with melodia-ui.pot, so {RUN_THE_SCRIPT}: {out_of_step:?}"
    );
}

#[test]
fn no_catalogue_ships_a_string_in_english() {
    let mut in_english = Vec::new();
    for (code, entries) in catalogues() {
        let skipped = entries.iter().filter(|entry| entry.shows_in_english());
        in_english.extend(skipped.map(|entry| format!("{code}: \"{}\"", entry.msgid)));
    }
    assert!(
        in_english.is_empty(),
        "these are fuzzy or untranslated, so Slint shows them in English. Translate each and \
         drop any `#, fuzzy` line: {in_english:?}"
    );
}

#[test]
fn every_translator_note_reaches_the_template() {
    let (tree, misplaced) = slint_tree_notes();
    assert!(
        misplaced.is_empty(),
        "these notes don't sit directly above a line holding exactly one `@tr(`, so the \
         extractor drops them or hands them to every call on that line: {misplaced:?}"
    );
    assert!(tree.len() >= MIN_NOTES, "only {} noted msgids found under {UI_DIR}", tree.len());

    let template = template_notes(&template_entries());
    let unshipped = unmatched(&tree, &template);
    let stale = unmatched(&template, &tree);
    assert!(
        unshipped.is_empty() && stale.is_empty(),
        "translator notes out of step with melodia-ui.pot, so {RUN_THE_SCRIPT}. If it already \
         ran, an earlier comment above the same string in that file took the note's place, \
         since the extractor keeps the first. Not in the template: {unshipped:?}. \
         Not in the tree: {stale:?}"
    );
}

/// English is the source baseline and ships no catalogue; every other supported
/// code must have one, or `select_bundled_translation` silently leaves the UI in
/// English for that pick.
#[test]
fn every_supported_locale_but_english_ships_a_catalogue() {
    for code in SUPPORTED_LOCALES {
        let path = catalogue_path(code);
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
/// The class the msgid pins structurally cannot see: an unwrapped literal declares no msgid,
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
            // `trim_end` rather than an exact prefix: a `@tr(` can put its literal on the
            // next line.
            if src.get(..quote).is_some_and(|head| head.trim_end().ends_with("@tr(")) {
                continue;
            }
            let shown = read_literal(&src, quote).map_or_else(String::new, |(body, _)| body.into());
            offenders.push(format!("{path}: \"{shown}\""));
        }
    }

    assert!(
        offenders.is_empty(),
        "{offenders:?} paint an untranslated fallback. Wrap it in `@tr(…)` and {RUN_THE_SCRIPT}, \
         reusing `Unknown artist` / `Unknown album` verbatim rather than title-casing a second \
         entry that says the same thing."
    );
}
