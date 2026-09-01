//! Native-dialog pin: nothing in the tree builds an `rfd` dialog for itself.
//!
//! It walks the Rust sources rather than naming the five call sites, because what it
//! guards against is a *sixth*: a dialog is a handful of builder calls, so the temptation
//! is to spell the constructor inline — which works, and is wrong in a way no review on
//! this machine can see. A pin over a fixed list is the pin that new site walks past.
//!
//! `src/` is the whole reach it needs: `melodia-ui` depends on `slint` alone, so the
//! other package has no `rfd` to build a dialog with.

use crate::test_support::{MIN_SOURCES, SRC_DIR, stripped_sources};

/// The two names nothing outside the helper may spell: the crate the dialogs come from,
/// and the parenting call the helper exists for.
///
/// The crate rather than `AsyncFileDialog`, because the blocking `rfd::FileDialog` ships
/// unparented just as readily and would need a needle of its own — where naming *any*
/// rfd type has to name the crate, in a path or a `use`.
const RAW_CRATE: &str = "rfd";
const RAW_PARENT: &str = "set_parent";

/// What a call site says instead. Module-qualified, which is how all five spell it — a
/// `use …::parented` reads as one caller fewer here.
const HELPER: &str = "file_dialog::parented(";

/// The two files that may name the raw calls, and the name each must **still** spell.
/// The helper owes `set_parent`: that one call is the whole module, it is the half no
/// review on this machine can see missing, and nothing else is positioned to notice it
/// go. This pin owes the crate name, being the needle.
const EXEMPT: [(&str, &str); 2] = [
    ("ui/file_dialog.rs", RAW_PARENT),
    ("ui/tests/file_dialog_tests.rs", RAW_CRATE),
];

/// A floor rather than an equality, so a genuine sixth dialog needs no edit here — but a
/// caller that stops opening one, or a walk that silently found nothing, still trips it.
const MIN_CALLERS: usize = 5;

/// The whole Rust tree, comment-stripped and paired with the path it came from. Stripped
/// because prose about the rule reads exactly like a violation of it — this file's own
/// header would otherwise be the first hit.
fn sources() -> Vec<(String, String)> {
    stripped_sources(SRC_DIR, "rs", MIN_SOURCES)
}

/// The parenting is what stops the OS picker opening *behind* Melodia on Windows
/// and macOS, and it is unobservable on Linux — the XDG portal parents OS-side
/// whatever we hand it. So a call site that builds its own dialog is correct on
/// the platform it is written and reviewed on, and wrong on the two it is not.
/// Reaching for the helper is the whole guarantee; this is the check that it was
/// reached for.
#[test]
fn every_native_dialog_is_built_by_the_shared_helper() {
    let mut callers = Vec::new();
    let mut inline = Vec::new();
    let mut exempt_seen = Vec::new();

    for (path, src) in sources() {
        // Skipped rather than merely forgiven the raw calls, for a different reason
        // each: this pin spells `HELPER` as its needle, so counting it would inflate the
        // floor past a caller that genuinely stopped opening a dialog, and the helper is
        // the one site that owns the calls. What each still owes is `EXEMPT`'s second
        // column.
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
        if src.contains(RAW_CRATE) || src.contains(RAW_PARENT) {
            inline.push(path);
        }
    }

    assert!(
        inline.is_empty(),
        "{inline:?} build a native dialog by hand — use `ui::file_dialog::parented`, \
         which is what parents it to the main window"
    );
    assert!(
        callers.len() >= MIN_CALLERS,
        "only {} native-dialog caller(s) found ({callers:?}); expected at least \
         {MIN_CALLERS} — one stopped opening a dialog, one stopped spelling \
         `{HELPER}…)` for it, or the walk is broken",
        callers.len()
    );
    assert_eq!(
        exempt_seen.len(),
        EXEMPT.len(),
        "EXEMPT names {EXEMPT:?} but the walk only reached {exempt_seen:?} — a moved or \
         renamed entry pre-authorises whatever takes its path next"
    );
}
