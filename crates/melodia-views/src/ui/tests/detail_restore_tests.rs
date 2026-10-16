//! Source-level pins on the boot restore of a detail view.
//!
//! A restore lands its id only when the fetch behind it returns, which is long after the first
//! paint, so for that gap every `*-open` predicate reads false and the mounted tab's **grid** is
//! what a boot onto a detail paints. `restoring` is the term that suppresses it, and it is worth
//! pinning because the two halves sit in different trees and neither reads wrong without the
//! other: the flag is raised in Rust before `app.show()`, and spent in Slint on a branch
//! condition per tab.
//!
//! The gap itself is as old as the seed. What made it visible was the fade coming off the grid's
//! own mount, so the failure mode is a page that reads correctly in review and flashes the wrong
//! content on every launch.
//!
//! **Five seeds, two sheets.** The Rust half is one contract and every seed is held against it
//! here. The Slint half is not shared: My Library folds its four details into one
//! `detail-restoring` predicate its own sheet declares, where Radio's three tab bodies read
//! `Radio.restoring` directly, so each sheet is walked on its own terms.

use melodia_testkit::{block_after, strip_line_comments, tab_body_branches};

const SHEET: &str = include_str!("../../../../melodia-ui/ui/views/my-library-view.slint");
const RADIO_SHEET: &str = include_str!("../../../../melodia-ui/ui/views/radio-view.slint");

/// The tabs Radio declares today, kept local for `radio_tests::TABS`' reason: a change to
/// `Radio.tab-count` must not silently rewrite what this asserts.
const RADIO_TABS: usize = 3;

/// The four grid tabs, as `(tab constant, open predicate, detail global)`. Songs is absent on
/// purpose: it drills into nothing, so it has no restore to wait for.
const DETAIL_TABS: [(&str, &str, &str); 4] = [
    ("tab-albums", "album-open", "AlbumDetail"),
    ("tab-artists", "artist-open", "ArtistDetail"),
    ("tab-genres", "genre-open", "GenreDetail"),
    ("tab-playlists", "playlist-open", "PlaylistDetail"),
];

/// The five seeds, each named for the failure message beside the function that raises its flag
/// and the open it waits on. Radio is not a My Library tab and its flag lives on its own global,
/// but the Rust half of the contract is the same one and it is the half a seed gets wrong. The four
/// My Library seeds reopen through `restore_detail`, which a hidden tab's first enter calls as well.
const SEEDS: [(&str, &str, &str, &str); 5] = [
    ("albums", include_str!("../albums/detail.rs"), "pub fn restore_detail", "open_album("),
    ("artists", include_str!("../artists/detail.rs"), "pub fn restore_detail", "open_artist("),
    ("genres", include_str!("../genres/detail.rs"), "pub fn restore_detail", "open_genre("),
    (
        "playlists",
        include_str!("../playlists/detail.rs"),
        "pub fn restore_detail",
        "open_playlist(",
    ),
    (
        "radio",
        include_str!("../radio/detail.rs"),
        "pub fn seed_detail_from_settings",
        "open_station(",
    ),
];

/// The launch's seed and the first enter's resume, shared by the four My Library details.
const RESTORE: &str = include_str!("../track_detail/restore.rs");

/// The four My Library tabs' section gates.
const LIFECYCLES: [(&str, &str); 4] = [
    ("albums", include_str!("../albums/callbacks/lifecycle.rs")),
    ("artists", include_str!("../artists/callbacks/lifecycle.rs")),
    ("genres", include_str!("../genres/callbacks/lifecycle.rs")),
    ("playlists", include_str!("../playlists/callbacks/lifecycle.rs")),
];

/// The sheet with its comments dropped, so prose about the fix can't satisfy a pin.
fn sheet() -> String {
    strip_line_comments(SHEET)
}

/// The line mounting `tab`'s grid, found by the pair of terms only that branch carries.
fn grid_branch<'a>(sheet: &'a str, tab: &str, open: &str) -> &'a str {
    let tab_term = format!("MyLibrary.tab-idx == MyLibrary.{tab}");
    let open_term = format!("!root.{open}");
    sheet
        .lines()
        .find(|line| line.contains(&tab_term) && line.contains(&open_term))
        .unwrap_or_default()
}

/// The body of the function `entry` opens, or the empty string where the pin has lost it.
fn restore_body(source: &str, entry: &str) -> String {
    block_after(&strip_line_comments(source), entry).to_owned()
}

/// A grid mounted while its own detail is still being fetched paints the page the user is
/// arriving *from*, for as long as that fetch takes.
#[test]
fn every_grid_branch_waits_for_a_pending_restore() {
    let sheet = sheet();
    for (tab, open, _) in DETAIL_TABS {
        let branch = grid_branch(&sheet, tab, open);
        assert!(
            branch.contains("!root.detail-restoring"),
            "{tab}'s grid mounts without waiting for a restore: {branch:?}"
        );
    }
}

/// The predicate carries each detail behind its own tab check, which is what lets all four
/// branches read the one property and still reduce to their own view's flag. A detail missing
/// from it leaves that one tab flashing while its three siblings look fixed.
#[test]
fn the_predicate_answers_for_every_detail_the_page_drills_into() {
    let sheet = sheet();
    let predicate = sheet
        .split_once("property <bool> detail-restoring:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(predicate, _)| predicate);

    for (tab, _, global) in DETAIL_TABS {
        let arm = format!("MyLibrary.tab-idx == MyLibrary.{tab} && {global}.restoring");
        assert!(predicate.contains(&arm), "`detail-restoring` is missing `{arm}`: {predicate:?}");
    }
}

/// **The raise has to be synchronous.** A launch's reopen runs inside `install_views`, well before
/// `app.show()`, and that is the whole window it has: moved inside the spawn it lands after the
/// first paint and the flash comes back with every other line of the fix still in place.
#[test]
fn every_seed_raises_the_flag_before_it_spawns_the_fetch() {
    for (view, source, entry, _) in SEEDS {
        let seed = restore_body(source, entry);
        let raise = seed.find("set_restoring(true)").unwrap_or(usize::MAX);
        // `.spawn(`, not `runtime.spawn(`: a seed that moves its own `state` into the block has to
        // clone the `Arc<Runtime>` out first, so the receiver is not a fixed spelling. Narrowed
        // back, the test reads `0` for those and fails on a seed that is correct.
        let spawn = seed.find(".spawn(").unwrap_or(0);
        assert!(
            raise < spawn,
            "{view} must raise `restoring` above the spawn, or the flag goes up after first paint"
        );
    }
}

/// A raise with no lower is worse than the flash it replaced: the body then mounts nothing at
/// all, and keeps mounting nothing for the rest of the session. The single call sits past the
/// fetch so it covers the failing arm too, a detail deleted since the last session owing the
/// grid back rather than an empty page.
#[test]
fn every_seed_lowers_the_flag_again_whatever_the_fetch_did() {
    for (view, source, _, _) in SEEDS {
        assert_eq!(
            source.matches("set_restoring(false)").count(),
            1,
            "{view} must hand `restoring` back exactly once, off the end of the fetch"
        );
    }
}

/// Lowered ahead of the open's own hop, the flag hands the grid back for the length of the fetch,
/// and the arrival latch reads a fall with nothing open as a restore that found nothing, so the
/// detail then arrives with the body fade the restore exists to suppress.
#[test]
fn every_restore_lowers_the_flag_only_after_its_open() {
    for (view, source, entry, open) in SEEDS {
        let restore = restore_body(source, entry);

        let opened = restore.find(open);
        let lowered = restore.find("set_restoring(false)");

        assert!(
            opened.is_some() && lowered > opened,
            "{view} lowers `restoring` at {lowered:?}, not after `{open}` at {opened:?}"
        );
    }
}

/// A launch onto the tab its detail sits on reopens it straight away; behind another tab it
/// would publish into globals another tab is painting.
#[test]
fn the_launch_reopens_the_shown_tabs_detail_at_once() {
    let restore = strip_line_comments(RESTORE);
    let seed = block_after(&restore, "pub fn seed_or_defer");

    assert!(
        seed.trim_end().ends_with("restore(ui, state, view, id);"),
        "the seed no longer ends by reopening the shown tab's detail: {seed}"
    );
}

/// A hidden tab's seed parks the id and stops there, or the launch decodes a hero and covers for
/// a detail nothing draws.
#[test]
fn a_hidden_tab_reopens_nothing_at_launch() {
    let restore = strip_line_comments(RESTORE);
    let seed = block_after(&restore, "pub fn seed_or_defer");

    let hidden = block_after(seed, "if !tab_is_mounted(ui, V::TAB)");

    assert!(hidden.contains("return;"), "the hidden arm falls through to the reopen: {hidden}");
}

/// What the first enter is handed it reopens, and a detail a drill opened first keeps it from
/// being handed anything.
#[test]
fn the_first_enter_reopens_what_the_launch_parked() {
    let restore = strip_line_comments(RESTORE);
    let resume = block_after(&restore, "pub fn resume_deferred");

    let read = (
        resume.contains("take_deferred_restore(|| view.detail_open())"),
        resume.contains("restore(&ui, state, view, id);"),
    );

    assert_eq!(read, (true, true), "(asks whether a detail is open, reopens): {resume}");
}

/// A gate's enter arm is the tab's first enter; in the leave arm the parked reopen would land on a
/// tab the user just left.
#[test]
fn every_gate_resumes_on_its_enter_arm() {
    for (view, lifecycle) in LIFECYCLES {
        let lifecycle = strip_line_comments(lifecycle);

        let enter = block_after(&lifecycle, "if active {");

        assert!(enter.contains("restore::resume_deferred("), "{view}'s enter arm: {enter}");
    }
}

/// Radio's Slint half: **each of the three tab bodies waits for the restore on its own.**
///
/// The page routes on `(tab-idx, detail-open)` and its station page is per-tab in Rust, so there
/// is no shared predicate to hold the term once. A tab body left off it flashes its grid for the
/// length of the fetch while the two beside it look fixed, and only on the tab the restore is
/// landing on — which is the one the reviewer is least likely to be sitting on.
#[test]
fn every_radio_tab_body_waits_for_a_pending_restore() {
    let branches = tab_body_branches(RADIO_SHEET, "Radio");

    assert_eq!(branches.len(), RADIO_TABS, "expected one body branch per tab: {branches:?}");
    for branch in branches {
        assert!(
            branch.contains("!Radio.restoring"),
            "a radio tab body mounts without waiting for a restore: {branch:?}"
        );
    }
}
