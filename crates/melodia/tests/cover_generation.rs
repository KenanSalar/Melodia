//! Pins for the half of the scheduling contract the compiler can't see.
//!
//! A lazy cover lookup that answers a miss by scheduling is only correct if something later
//! re-runs the binding that missed. Slint enforces the *callback* signature, so a scheduling
//! lookup wired to a generation-less `on_request_cover` doesn't build — what compiles cleanly is
//! calling one from some *other* handler, or forgetting to install the notifier that does the
//! bumping. Both leave a placeholder nothing ever replaces, which reads as a cold tier and shows
//! up only on a library big enough to miss.

use std::collections::BTreeSet;

use melodia_testkit::rust_sources;

/// `grid_cover`'s own pin, which passes both generations as literals — exercising the cold arm
/// against the warm one is what it is for, and neither is a call site that has to come back.
const EXEMPT: &str = "ui/tests/grid_prewarm_tests.rs";

/// A floor under the lookup walk, standing in for the self-check this pin carried while it lived
/// inside the corpus it walks: it skipped itself and asserted it still spelled the needle. Out
/// here there is nothing to skip, and a renamed helper would otherwise empty the walk in silence.
const MIN_LOOKUPS: usize = 8;

/// Every file that installs a decoded-batch notifier, one per tier a scheduling lookup reads.
///
/// **An equality, not a floor.** A tier whose notifier goes missing still compiles, still
/// schedules and still decodes — it simply never tells anyone, so its cards sit on the
/// placeholder until something unrelated dirties the binding. There is nothing to see in review
/// and nothing to catch at runtime.
const NOTIFIER_HOMES: [&str; 8] = [
    "boot/ui_setup/views.rs",
    "ui/albums/callbacks/grid.rs",
    "ui/artists/callbacks/grid.rs",
    "ui/browse/mod.rs",
    "ui/favorites/mod.rs",
    "ui/playlists/callbacks/grid.rs",
    "ui/radio/mod.rs",
    "ui/recently_played/mod.rs",
];

/// `grid_cover` schedules on a miss, so every call owes a generation on the same line — either
/// the one its callback was handed or the one it forwards. A call without one is a card that
/// resolves only if the tier happened to be warm.
///
/// Definitions are skipped: the three per-view methods take the path alone and read the tier's
/// own scheduling lookup, which is the point. `grid_cover_blocking` is the deliberate sibling for
/// a surface with no generation to come back on and doesn't match this needle.
#[test]
fn every_scheduling_cover_lookup_names_a_generation() {
    let mut seen = 0_usize;
    let mut offenders = Vec::new();

    for (path, code) in rust_sources() {
        if path == EXEMPT {
            continue;
        }
        for line in code.lines() {
            if !line.contains("grid_cover(") || line.contains("fn grid_cover(") {
                continue;
            }
            seen += 1;
            if !line.contains("generation") {
                offenders.push(format!("{path}: {}", line.trim()));
            }
        }
    }

    assert!(
        seen >= MIN_LOOKUPS,
        "only {seen} scheduling cover lookups found; a renamed helper empties this walk and          every card it guards goes unchecked"
    );
    assert!(
        offenders.is_empty(),
        "a scheduling cover lookup with no generation on the line leaves the card on its \
         placeholder for good. Use `grid_cover_blocking` where the surface genuinely has no \
         generation — a one-shot dialog slot, or a strip whose callback carries none:\n{}",
        offenders.join("\n")
    );
}

/// Every tier a scheduling lookup reads gets told when its batch lands.
#[test]
fn every_scheduling_tier_installs_a_notifier() {
    let mut found = BTreeSet::new();

    for (path, code) in rust_sources() {
        // The definition's own file, which names it without installing anything.
        if path == "ui/cover_generation.rs" {
            continue;
        }
        if code.contains("notify_on_decode(") {
            found.insert(path);
        }
    }

    let expected: BTreeSet<String> = NOTIFIER_HOMES.iter().map(|&s| s.to_owned()).collect();
    assert_eq!(
        found, expected,
        "the set of files installing a decoded-batch notifier has moved. A *missing* entry is a \
         tier whose scheduled decodes never reach the screen; an *extra* one is a tier this list \
         hasn't been told about."
    );
}
