//! Pins for the half of the scheduling contract the compiler can't see.
//!
//! A lazy cover lookup that answers a miss by scheduling is only correct if something later
//! re-runs the binding that missed. Slint enforces the *callback* signature, so a scheduling
//! lookup wired to a generation-less `on_request_cover` doesn't build — what compiles cleanly is
//! calling one from some *other* handler, or forgetting to install the notifier that does the
//! bumping. Both leave a placeholder nothing ever replaces, which reads as a cold tier and shows
//! up only on a library big enough to miss.

use std::collections::{BTreeMap, BTreeSet};

use melodia_testkit::rust_sources;

/// The two surfaces sanctioned to decode inline, and the only files `grid_cover_blocking` may be
/// named in besides its own definition and its own test.
///
/// **An equality, not a floor**, and each entry is asserted to still match something: a third
/// site is one screenful of grid-tier decodes on the event loop, and an entry that has stopped
/// matching is a hole that reads as coverage. Artist Detail's Albums strip and the Edit Artwork
/// dialog's cover slot both lack a generation to come back on — the strip's callback carries
/// none, the dialog's slot is a one-shot property write — so a scheduled cover there would be a
/// placeholder nothing ever replaces.
const BLOCKING_LOOKUP_SITES: [&str; 2] =
    ["ui/artists/callbacks/cross_tab.rs", "ui/playlists/callbacks/dialog.rs"];

/// A floor under the lookup walk, standing in for the self-check this pin carried while it lived
/// inside the corpus it walks: it skipped itself and asserted it still spelled the needle. Out
/// here there is nothing to skip, and a renamed helper would otherwise empty the walk in silence.
const MIN_LOOKUPS: usize = 6;

/// Every file that installs a decoded-batch notifier, and **how many it installs**.
///
/// **An equality, not a floor**, and the count is half of it: the boot file installs two, the row
/// tier's and the grid tier's, so a set of *files* would find it either way and the one that
/// matters most could go missing in silence. `set_decoded_notifier` is a `OnceLock`, so that
/// single grid install is what bumps all six grid generations; Radio's logo tier is the one card
/// tier that stays private — its station tile reads the decoded extent — so it keeps its own.
/// A tier whose notifier goes missing still compiles, still schedules and still decodes — it
/// simply never tells anyone, so its cards sit on the placeholder until something unrelated
/// dirties the binding.
const NOTIFIER_HOMES: [(&str, usize); 2] = [("boot/ui_setup/views.rs", 2), ("ui/radio/mod.rs", 1)];

/// Every repeated cover surface takes the **scheduling** lookup, and the inline one stays at its
/// two sanctioned sites.
///
/// `grid_cover` hands a miss to the decode pool and answers with the placeholder, which the
/// shared notifier then replaces; `grid_cover_blocking` decodes on the calling thread, and the
/// calling thread is the event loop. Swapping one for the other at a grid compiles, reviews
/// cleanly, and freezes the frame that mounts the grid on a library big enough to miss.
#[test]
fn only_the_two_generation_less_surfaces_decode_inline() {
    let mut seen = 0_usize;
    let mut offenders = Vec::new();

    for (path, code) in rust_sources() {
        for line in code.lines() {
            if !line.contains("grid_cover") || line.contains("fn grid_cover") {
                continue;
            }
            seen += 1;
            if line.contains("grid_cover_blocking")
                && !BLOCKING_LOOKUP_SITES.contains(&path.as_str())
            {
                offenders.push(format!("{path}: {}", line.trim()));
            }
        }
    }

    assert!(
        seen >= MIN_LOOKUPS,
        "only {seen} cover lookups found; a renamed helper empties this walk and every card it \
         guards goes unchecked"
    );
    assert!(
        offenders.is_empty(),
        "an inline cover decode outside the two surfaces that need one puts a decode per visible \
         card on the event loop. Use `grid_cover`, which schedules:\n{}",
        offenders.join("\n")
    );

    // Every exemption still matches something — an entry that has stopped applying is a hole
    // that reads as coverage.
    for site in BLOCKING_LOOKUP_SITES {
        assert!(
            rust_sources()
                .into_iter()
                .any(|(path, code)| path == site && code.contains("grid_cover_blocking")),
            "`{site}` is exempted from the inline-decode ban but no longer names \
             `grid_cover_blocking` — drop the entry rather than leaving it standing"
        );
    }
}

/// Every tier a scheduling lookup reads gets told when its batch lands.
#[test]
fn every_scheduling_tier_installs_a_notifier() {
    let mut found = BTreeMap::new();

    for (path, code) in rust_sources() {
        // The definition's own file, which names it without installing anything.
        if path == "ui/cover_generation.rs" {
            continue;
        }
        let installs = code.matches("notify_on_decode(").count();
        if installs > 0 {
            found.insert(path, installs);
        }
    }

    let expected: BTreeMap<String, usize> =
        NOTIFIER_HOMES.iter().map(|&(path, n)| (path.to_owned(), n)).collect();
    assert_eq!(
        found, expected,
        "the set of decoded-batch notifier installs has moved. A *missing* one is a tier whose \
         scheduled decodes never reach the screen; an *extra* one is a tier this list hasn't been \
         told about. The count is what separates the boot file's two."
    );
}

/// Every file that builds a `CoverThumbs` of its own, and what makes each one's a separate tier.
///
/// **An equality, not a floor**, because a sixth costs memory in exactly the way the seven this
/// collapsed did: each holds a screenful of decoded RGB8, and those seven — in six view handles,
/// Favorites holding two — were the same LRU at the same size with the same cap, split only so a
/// section leave could release its own. That is what `grid_prewarm::hand_back_covers` replaced. A
/// new tier here is either a decode size nothing else draws at, or a regression — and a passing
/// suite is the last place it would show.
const TIER_HOMES: [(&str, &str); 5] = [
    ("boot/ui_setup/views.rs", "the row tier, at row-tile size"),
    ("ui/grid_prewarm.rs", "the one tier every card grid draws from"),
    ("ui/queue_sheet/mod.rs", "the sheet's own, dropped on close rather than evicted"),
    ("ui/radio/covers.rs", "logos, whose card reads the decoded extent for its layout"),
    ("ui/search/mod.rs", "the two card strips, at strip sizes of their own"),
];

#[test]
fn only_the_named_cover_tiers_are_built() {
    let mut found = BTreeSet::new();

    for (path, code) in rust_sources() {
        // A test builds its own tier to exercise one and holds it for the length of the test, so
        // this is a question about production code alone. By segment because a crate's top-level
        // `src/tests/` arrives as `tests/…`, which `/tests/` does not match.
        if path.split('/').any(|segment| segment == "tests") {
            continue;
        }
        // The type's own module, which names its constructors without calling them.
        if path == "media/image/cover_thumbs.rs" {
            continue;
        }
        if code.contains("CoverThumbs::with_config(") || code.contains("CoverThumbs::new()") {
            found.insert(path);
        }
    }

    let expected: BTreeSet<String> =
        TIER_HOMES.iter().map(|(path, _)| (*path).to_owned()).collect();
    assert_eq!(
        found,
        expected,
        "the set of files building a cover tier has moved. An *extra* one is a screenful of \
         decoded covers this list hasn't been told about — say what decode size it draws at that \
         no existing tier does, or route it through `grid_prewarm::tier()`. A *missing* one is a \
         tier that has stopped existing and an entry to drop:\n  {}",
        TIER_HOMES.map(|(path, why)| format!("{path} — {why}")).join("\n  ")
    );
}

/// The six card grids one notifier answers for, and the three files allowed to move a
/// `covers-generation` at all.
///
/// Radio is absent from the first on purpose: its logo tier is its own, because
/// `station-card.slint` reads the decoded *extent* for its layout. It is present in the second for
/// the same reason, beside the queue sheet's private tier. A seventh grid whose global never gets
/// bumped schedules its decodes and then sits on the placeholder until the next column change.
const GRID_GENERATION_GLOBALS: [&str; 6] =
    ["Albums", "Artists", "Browse", "Favorites", "Playlists", "RecentlyPlayed"];

const GENERATION_WRITERS: [&str; 3] =
    ["boot/ui_setup/views.rs", "ui/queue_sheet/callbacks.rs", "ui/radio/covers.rs"];

#[test]
fn only_the_boot_install_answers_for_the_grid_tier() {
    let sources = rust_sources();
    let boot = sources
        .iter()
        .find(|(path, _)| path == "boot/ui_setup/views.rs")
        .map_or("", |(_, src)| src.as_str());

    let install = melodia_testkit::block_after(boot, "fn install_grid_covers(");
    let repainted: BTreeSet<&str> = install
        .match_indices("melodia_ui::")
        .filter_map(|(at, needle)| {
            let rest = install.get(at + needle.len()..)?;
            let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
            rest.get(..end)
        })
        .collect();

    assert_eq!(
        repainted,
        GRID_GENERATION_GLOBALS.into_iter().collect::<BTreeSet<&str>>(),
        "`set_decoded_notifier` is a `OnceLock`, so this closure is the only thing that brings a \
         landed grid decode to the screen. A bump on an unmounted page costs nothing, so leaving \
         a global out buys nothing and strands its cards on the placeholder"
    );

    let writers: BTreeSet<&str> = sources
        .iter()
        .filter(|(_, src)| src.contains("set_covers_generation("))
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(
        writers,
        GENERATION_WRITERS.into_iter().collect::<BTreeSet<&str>>(),
        "a repaint token moved from anywhere else is a second answer to `is this tier warm`, and \
         the two only ever disagree"
    );
}
