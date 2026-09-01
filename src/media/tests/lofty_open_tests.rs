//! Single-opener pin: nothing outside `media::metadata` hands a path to lofty.
//!
//! A walk rather than the two call sites, because what it guards is a third. Lofty's own
//! openers key on the extension and its map has no entry for `.oga`, so a site spelling one
//! reads correctly and refuses a file the scan read by its header: `tag_writer` did exactly
//! that, and the rows it could not save were the ones the library had listed in full.
//!
//! `src/` is the whole reach it needs — `melodia-ui` depends on `slint` alone, so the other
//! package has no lofty to open a file with.

use crate::test_support::{MIN_SOURCES, SRC_DIR, stripped_sources};

/// What an exempt file has to keep spelling: the type rather than the module path, so it holds
/// under either import shape.
const RAW_PROBE_TYPE: &str = "probe::Probe";

/// Every spelling that reaches lofty's own openers, and it takes three: `probe::Probe` is what a
/// grouped `use lofty::{probe::Probe, …}` leaves behind, `lofty::probe` what a glob import does,
/// and neither is a substring of the other. `read_from` catches `read_from_path` as a prefix,
/// both being crate-root re-exports a call can reach naming no module at all.
const RAW_OPENERS: [&str; 3] = [RAW_PROBE_TYPE, "lofty::probe", "read_from"];

/// What a call site says instead. Unqualified, `tag_writer` reaching it through a module path
/// and the tag-writer tests through a `use`.
const HELPER: &str = "read_tags(";

/// The two files that may name the raw openers, and the name each must **still** spell.
/// `metadata.rs` owes the probe: the sniff wrapped around it is the whole module, and nothing
/// else is positioned to notice it go. This pin owes the needle, being the needle.
const EXEMPT: [(&str, &str); 2] = [
    ("media/metadata.rs", RAW_PROBE_TYPE),
    ("media/tests/lofty_open_tests.rs", RAW_PROBE_TYPE),
];

/// A floor rather than an equality, so a genuine third caller needs no edit here — but a
/// caller that stops reading tags, or a walk that silently found nothing, still trips it.
const MIN_CALLERS: usize = 2;

/// Lofty resolves a format from the extension and stops there, which is a narrower question
/// than its parsers answer. `read_tags` asks the header when the extension resolved to
/// nothing; a site that opens its own probe gets the narrow answer and no indication it took
/// one. Reaching for the helper is the whole guarantee, and this is the check it was reached
/// for.
#[test]
fn every_lofty_open_goes_through_the_shared_reader() {
    let mut callers = Vec::new();
    let mut inline = Vec::new();
    let mut exempt_seen = Vec::new();

    // Comment-stripped, because prose about the rule reads exactly like a violation of it —
    // this file's own header would otherwise be the first hit.
    for (path, src) in stripped_sources(SRC_DIR, "rs", MIN_SOURCES) {
        if let Some((_, owed)) = EXEMPT.iter().find(|(exempt, _)| *exempt == path) {
            assert!(
                src.contains(owed),
                "{path} no longer names `{owed}`, so nothing is checking it any more"
            );
            exempt_seen.push(path);
            continue;
        }

        if src.contains(HELPER) {
            callers.push(path.clone());
        }
        if RAW_OPENERS.iter().any(|opener| src.contains(opener)) {
            inline.push(path);
        }
    }

    assert!(
        inline.is_empty(),
        "{inline:?} open a file with lofty directly — use `media::metadata::read_tags`, which \
         falls back to the header for the extensions lofty's own map has no entry for"
    );
    assert!(
        callers.len() >= MIN_CALLERS,
        "only {} file(s) reach the shared reader ({callers:?}); expected at least \
         {MIN_CALLERS} — one stopped reading tags, one stopped spelling `{HELPER}…)` for it, \
         or the walk is broken",
        callers.len()
    );
    assert_eq!(
        exempt_seen.len(),
        EXEMPT.len(),
        "EXEMPT names {EXEMPT:?} but the walk only reached {exempt_seen:?} — a moved or \
         renamed entry pre-authorises whatever takes its path next"
    );
}
