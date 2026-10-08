//! Pins the translation catalogues to the Slint tree `scripts/update-translations.sh` generates
//! them from, and to the bundled faces that draw them.
//!
//! It walks the sources rather than pinning a list, so a new locale extends
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

use melodia_core::entities::locale::{DEFAULT_LOCALE, SUPPORTED_LOCALES};
use melodia_testkit::{
    MIN_SLINT_SOURCES, REPO_ROOT, UI_DIR, font_sources, raw_sources, rel_path, strip_line_comments,
    stripped_sources,
};
use ttf_parser::Face;

const TRANSLATIONS_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-ui/translations");

/// Floor under every msgid count here, the tree's and each parsed file's, so a broken walk or
/// parser can't pass vacuously. Loose on purpose; it only catches a traversal or a parse that
/// found next to nothing.
const MIN_MSGIDS: usize = 400;

/// Floor under the plural pairs found in the tree, loose for [`MIN_MSGIDS`]' reason.
const MIN_PLURALS: usize = 10;

/// Floor under the msgids a translator note sits on in the tree, loose for [`MIN_MSGIDS`]' reason.
const MIN_NOTES: usize = 20;

/// The three weights `app-window.slint` imports. A floor so a weight added later is checked with
/// no edit here, and tight so one dropping out of the walk is not missed.
const MIN_TEXT_FACES: usize = 3;

const RUN_THE_SCRIPT: &str = "run scripts/update-translations.sh";

/// msgid → `msgid_plural`, the plural being there for the `@tr("one" | "many" % n)` form.
type Msgids = BTreeMap<String, Option<String>>;

/// msgid → its translator notes. A set because the same note can sit above a msgid in more than
/// one file, and the template carries it once.
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
    /// Whether a translator still owes this entry a string. Slint loads only what rspolib's
    /// `translated()` accepts, which rejects a plural with any form empty, so a half-done plural
    /// shows in English in every form.
    fn is_unfinished(&self) -> bool {
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

/// The `nplurals` `po`'s header declares, which is how many forms each of its plurals owes.
fn declared_plural_forms(po: &str) -> Option<usize> {
    let header: Vec<&str> = po.split("\n\n").next()?.lines().collect();
    let at = header.iter().position(|line| line.starts_with("msgstr "))?;
    let fields = read_po_value(&header, at, "msgstr")?;
    let plural_forms = fields.split("\\n").find_map(|field| field.strip_prefix("Plural-Forms:"))?;
    plural_forms
        .split(';')
        .filter_map(|setting| setting.split_once('='))
        .find(|(key, _)| key.trim() == "nplurals")
        .and_then(|(_, count)| count.trim().parse().ok())
}

/// Every plural in `entries` whose form count isn't `declared`, by msgid and the count it has.
fn plurals_off_count(entries: &[Entry], declared: usize) -> Vec<(&str, usize)> {
    entries
        .iter()
        .filter(|entry| entry.msgid_plural.is_some() && entry.msgstrs.len() != declared)
        .map(|entry| (entry.msgid.as_str(), entry.msgstrs.len()))
        .collect()
}

/// The arguments `text` substitutes, as Slint's formatter reads them: `{}` takes the next index,
/// `{0}` names one, `{n}` is a plural's count and `{{` is a literal brace. A translation may
/// reorder with indices, so this rather than the spelling is what it owes its msgid.
fn format_arguments(text: &str) -> BTreeSet<String> {
    let mut arguments = BTreeSet::new();
    let mut next_index = 0;
    let mut rest = text;
    while let Some((_, after)) = rest.split_once('{') {
        if let Some(escaped) = after.strip_prefix('{') {
            rest = escaped;
            continue;
        }
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        if name.is_empty() {
            arguments.insert(next_index.to_string());
            next_index += 1;
        } else {
            arguments.insert(name.trim().to_owned());
        }
        rest = tail;
    }
    arguments
}

/// Every translated form in `entries` substituting other arguments than its msgid, by msgid and
/// form index. A plural answers to `msgid_plural`, the English for every count but one.
fn forms_off_arguments(entries: &[Entry]) -> Vec<(&str, usize)> {
    let mut off = Vec::new();
    for entry in entries {
        let owed = format_arguments(entry.msgid_plural.as_deref().unwrap_or(&entry.msgid));
        for (form, msgstr) in entry.msgstrs.iter().enumerate() {
            if !msgstr.is_empty() && format_arguments(msgstr) != owed {
                off.push((entry.msgid.as_str(), form));
            }
        }
    }
    off
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

fn read_po(path: &Path) -> String {
    let po = fs::read_to_string(path);
    assert!(po.is_ok(), "can't read {}: {po:?}", path.display());
    po.unwrap_or_default()
}

/// `path` parsed, asserting it read and held at least [`MIN_MSGIDS`] entries, so a parser that
/// broke can't make every check over it pass on nothing.
fn read_entries(path: &Path) -> Vec<Entry> {
    let entries = parse_po(&read_po(path));
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
        .filter(|code| **code != DEFAULT_LOCALE)
        .map(|code| (*code, read_entries(&catalogue_path(code))))
        .collect()
}

/// The faces UI text is set in: what the font build writes under `vazirmatn/`. The icon faces
/// beside it draw ligatures, not the catalogues' text.
fn text_faces() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (fonts, unreadable) = font_sources();
    let faces = fonts
        .into_iter()
        .filter(|path| path.parent().is_some_and(|dir| dir.ends_with("vazirmatn")))
        .collect();
    (faces, unreadable)
}

/// Every character a locale can set from a catalogue, English's from the template, each with the
/// locales that set it.
///
/// Read still escaped, as every string here is: an escape is spelled in ASCII the faces carry, so
/// the escaped text answers for the unescaped.
fn shipped_characters() -> BTreeMap<char, BTreeSet<&'static str>> {
    let english = template_entries()
        .into_iter()
        .flat_map(|entry| iter::once(entry.msgid).chain(entry.msgid_plural))
        .map(|text| (DEFAULT_LOCALE, text));
    let translated = catalogues().into_iter().flat_map(|(code, entries)| {
        entries.into_iter().flat_map(|entry| entry.msgstrs).map(move |text| (code, text))
    });

    let mut characters: BTreeMap<char, BTreeSet<&'static str>> = BTreeMap::new();
    for (code, text) in english.chain(translated) {
        for character in text.chars() {
            characters.entry(character).or_default().insert(code);
        }
    }
    characters
}

/// One line per character in `characters` that `face` has no glyph for.
fn missing_glyphs(
    face: &Face<'_>,
    characters: &BTreeMap<char, BTreeSet<&str>>,
    face_name: &str,
) -> Vec<String> {
    characters
        .iter()
        .filter(|(character, _)| face.glyph_index(**character).is_none())
        .map(|(character, codes)| {
            format!(
                "{face_name} lacks {character} (U+{:04X}), set by {codes:?}",
                u32::from(*character)
            )
        })
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
fn no_catalogue_ships_a_fuzzy_or_empty_entry() {
    let mut unfinished = Vec::new();
    for (code, entries) in catalogues() {
        let owed = entries.iter().filter(|entry| entry.is_unfinished());
        unfinished.extend(owed.map(|entry| format!("{code}: \"{}\"", entry.msgid)));
    }
    assert!(
        unfinished.is_empty(),
        "these are fuzzy or untranslated, so Slint shows them in English, a plural with any form \
         left empty included. Translate each and drop any `#, fuzzy` line: {unfinished:?}"
    );
}

/// The shipped catalogues hold no fuzzy or empty entry, so the check above passes the same
/// whether the parser still spots one or not; only a hand-written catalogue can tell. A plural
/// with one form left empty is the row easiest to miss, since Slint shows every form in English.
#[test]
fn an_entry_is_unfinished_exactly_when_slint_shows_it_in_english() {
    const PO: &str = r#"msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"

msgid "translated"
msgstr "done"

msgid "empty"
msgstr ""

#, fuzzy
msgid "fuzzy"
msgstr "done"

#, c-format, fuzzy
msgid "fuzzy behind another flag"
msgstr "done"

#, c-format
msgid "a flag that isn't fuzzy"
msgstr "done"

msgid "wrapped"
msgstr ""
"done over "
"two lines"

msgid "{n} plural with every form"
msgid_plural "{n} plurals with every form"
msgstr[0] "one"
msgstr[1] "many"

msgid "{n} plural missing its second form"
msgid_plural "{n} plurals missing their second form"
msgstr[0] "one"
msgstr[1] ""

msgid "{n} plural missing its first form"
msgid_plural "{n} plurals missing their first form"
msgstr[0] ""
msgstr[1] "many"

msgid "{n} plural missing its third form"
msgid_plural "{n} plurals missing their third form"
msgstr[0] "one"
msgstr[1] "few"
msgstr[2] ""

msgid "no msgstr at all"
"#;

    let entries = parse_po(PO);

    let verdicts: Vec<(&str, bool)> =
        entries.iter().map(|entry| (entry.msgid.as_str(), entry.is_unfinished())).collect();
    assert_eq!(
        verdicts,
        [
            ("translated", false),
            ("empty", true),
            ("fuzzy", true),
            ("fuzzy behind another flag", true),
            ("a flag that isn't fuzzy", false),
            ("wrapped", false),
            ("{n} plural with every form", false),
            ("{n} plural missing its second form", true),
            ("{n} plural missing its first form", true),
            ("{n} plural missing its third form", true),
            ("no msgstr at all", true),
        ],
        "the parser disagrees with Slint about what ships in English, so the catalogue check \
         would pass an entry users see untranslated or fail one they don't"
    );
}

/// Slint picks a plural's form by its catalogue's `Plural-Forms` rule and shows the first form
/// where the entry has none at that index, so a Polish plural left at the template's two forms
/// prints the singular after "5" and nothing marks it unfinished. The script's `msgfmt -c` says
/// so, but only where the script runs.
#[test]
fn every_plural_carries_as_many_forms_as_its_catalogue_declares() {
    let mut mismatched = Vec::new();
    for (code, entries) in catalogues() {
        let Some(declared) = declared_plural_forms(&read_po(&catalogue_path(code))) else {
            mismatched.push(format!("{code}: the header declares no nplurals"));
            continue;
        };
        for (msgid, forms) in plurals_off_count(&entries, declared) {
            mismatched.push(format!("{code}: \"{msgid}\" has {forms} of {declared} forms"));
        }
    }
    assert!(
        mismatched.is_empty(),
        "Slint shows the singular for a form a plural lacks, so each owes exactly the forms its \
         catalogue's header declares: {mismatched:?}"
    );
}

/// Every shipped plural carries exactly its forms, so the check above passes the same whether it
/// asks for equality or only for enough; only a hand-written catalogue can tell. Indonesian is
/// where the two part: a plural copied from a two-form sibling keeps a second form its one-form
/// header never reads, which `msgfmt -c` rejects and a floor would pass.
#[test]
fn a_plural_owes_exactly_the_forms_its_header_declares() {
    const PO: &str = r#"msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Plural-Forms: nplurals=1; plural=0;\n"

msgid "not a plural"
msgstr "done"

msgid "{n} plural with its one form"
msgid_plural "{n} plurals with their one form"
msgstr[0] "every"

msgid "{n} plural copied from a two-form catalogue"
msgid_plural "{n} plurals copied from a two-form catalogue"
msgstr[0] "one"
msgstr[1] "many"
"#;

    assert_eq!(
        declared_plural_forms(PO),
        Some(1),
        "a one-form header read as anything else holds every plural to the wrong count"
    );
    assert_eq!(
        plurals_off_count(&parse_po(PO), 1),
        [("{n} plural copied from a two-form catalogue", 2)],
        "the form check must refuse a surplus form as well as a missing one, and leave a \
         singular entry alone"
    );
}

/// Slint substitutes an argument only where a translation spells its placeholder, so one left
/// out vanishes from the label and nothing marks the entry unfinished. In Russian it reads as a
/// wrong count rather than a missing one: the first plural form is shown after 21 and 31 as well
/// as 1, so a singular written without `{n}` claims one item for every count ending in 1.
#[test]
fn every_translation_substitutes_the_arguments_its_msgid_does() {
    let mut mismatched = Vec::new();
    for (code, entries) in catalogues() {
        for (msgid, form) in forms_off_arguments(&entries) {
            mismatched.push(format!("{code}: \"{msgid}\" form {form}"));
        }
    }
    assert!(
        mismatched.is_empty(),
        "these drop or add a placeholder, so Slint loses an argument or prints the braces. A \
         language that needs another order says so with `{{0}}` and `{{1}}`: {mismatched:?}"
    );
}

/// Every shipped translation keeps its arguments, so the check above passes the same whether it
/// still reads them as Slint does or not; only a hand-written catalogue can tell.
#[test]
fn a_translation_owes_its_msgids_arguments_and_not_their_spelling() {
    const PO: &str = r#"msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\n"

msgid "{} of {}"
msgstr "{1}: {0}"

msgid "{{braces}} kept as text"
msgstr "{{braces}} as text"

msgid "Saved to {}"
msgstr "Saved"

msgid "Folders"
msgstr "{} folders"

msgid "{n} file"
msgid_plural "{n} files"
msgstr[0] "one file"
msgstr[1] "{n} files"
msgstr[2] "{n} files"

msgid "{n} file left"
msgid_plural "{n} files left"
msgstr[0] "{n} file left"
msgstr[1] ""
msgstr[2] "{n} files left"
"#;

    assert_eq!(
        forms_off_arguments(&parse_po(PO)),
        [("Saved to {}", 0), ("Folders", 0), ("{n} file", 0)],
        "the argument check must refuse a dropped or surplus placeholder, a count missing from \
         a singular included, and pass an index reorder, an escaped brace and an empty form the \
         unfinished check owns"
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

/// Every note in the tree sits above a line with one call, so the placement check above passes
/// the same whether a shared line is still refused or not; only hand-written lines can tell.
#[test]
fn a_note_belongs_only_to_a_line_holding_exactly_one_tr_call() {
    const LINES: [(&str, Option<&str>); 4] = [
        (r#"text: @tr("Key");"#, Some("Key")),
        (r#"model: [@tr("Key"), @tr("Mode")];"#, None),
        ("text: root.key-label;", None),
        (r#"text: @tr("Key"); // was @tr("Mode")"#, Some("Key")),
    ];

    let found = LINES.map(|(line, _)| (line, sole_tr_call(line, 0, line)));

    assert_eq!(
        found, LINES,
        "the extractor hands a note to every `@tr(` on the line below it, so a shared line \
         can't place one, and a call inside a comment isn't a call"
    );
}

/// English is the source baseline and ships no catalogue; every other supported code needs one,
/// or `select_bundled_translation` silently leaves the UI in English for that pick. An equality
/// because [`catalogues`] reads the list: a catalogue dropped in without its entry is checked by
/// nothing here and offered by no picker.
#[test]
fn the_shipped_catalogues_are_exactly_the_supported_locales_but_english() {
    let mut shipped = BTreeSet::new();
    let mut unreadable = Vec::new();
    match fs::read_dir(TRANSLATIONS_DIR) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => {
                        let code = entry.file_name().to_string_lossy().into_owned();
                        if catalogue_path(&code).is_file() {
                            shipped.insert(code);
                        }
                    }
                    Err(e) => unreadable.push(e.to_string()),
                }
            }
        }
        Err(e) => unreadable.push(e.to_string()),
    }
    assert!(unreadable.is_empty(), "unreadable under translations/: {unreadable:?}");

    let supported: BTreeSet<String> = SUPPORTED_LOCALES
        .iter()
        .filter(|code| **code != DEFAULT_LOCALE)
        .map(|code| (*code).to_owned())
        .collect();
    assert_eq!(
        shipped, supported,
        "a locale is its SUPPORTED_LOCALES entry and its catalogue together"
    );
}

/// Every character a shipped locale sets, English included, has a glyph in each bundled text face.
///
/// One the faces lack is drawn from whatever face the OS falls back to, whose taller line box sets
/// the label off-centre in every pill and row, and nothing is drawn at all where the OS has no such
/// face. A Latin machine shows none of it, which is how the Greek catalogue shipped with nearly
/// every string in a fallback face. A locale whose alphabet the faces lack fails here until it has
/// a row in `scripts/patch_vazirmatn.py`'s `LANGUAGE_LETTERS` and the faces are rebuilt.
#[test]
fn every_character_a_locale_sets_is_in_the_bundled_faces() {
    let characters = shipped_characters();
    let (faces, unwalkable) = text_faces();
    assert!(unwalkable.is_empty(), "unreadable font directories: {unwalkable:?}");
    assert!(
        faces.len() >= MIN_TEXT_FACES,
        "only {} text faces found under vazirmatn/, so the check below covers fewer weights \
         than the UI sets text in",
        faces.len()
    );

    let mut unreadable = Vec::new();
    let mut missing = Vec::new();
    for path in &faces {
        let face_name = rel_path(REPO_ROOT, path);
        let data = match fs::read(path) {
            Ok(data) => data,
            Err(e) => {
                unreadable.push(format!("{face_name}: {e}"));
                continue;
            }
        };
        match Face::parse(&data, 0) {
            Ok(face) => missing.extend(missing_glyphs(&face, &characters, &face_name)),
            Err(e) => unreadable.push(format!("{face_name}: {e}")),
        }
    }

    assert!(unreadable.is_empty(), "text faces that won't read or parse: {unreadable:?}");
    assert!(
        missing.is_empty(),
        "these characters fall back to an OS face, off-centre wherever there is one and missing \
         where there isn't. Add the language's letters to `LANGUAGE_LETTERS` in \
         scripts/patch_vazirmatn.py and re-run it: {missing:#?}"
    );
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
