//! Source-level pins on the boot restore of a My Library detail.
//!
//! A restore lands its id only when the fetch behind it returns, which is long after the first
//! paint, so for that gap all four `*-open` predicates read false and the mounted tab's **grid**
//! is what a boot onto a detail paints. `<Detail>.restoring` is the term that suppresses it, and
//! it is worth pinning because the two halves sit in different trees and neither reads wrong
//! without the other: the flag is raised in Rust before `app.show()`, and spent in Slint on four
//! branch conditions.
//!
//! The gap itself is as old as the seed. What made it visible was the fade coming off the grid's
//! own mount, so the failure mode is a page that reads correctly in review and flashes the wrong
//! content on every launch.

use crate::test_support::strip_line_comments;

const SHEET: &str = include_str!("../../../melodia-ui/ui/views/my-library-view.slint");

/// The four grid tabs, as `(tab constant, open predicate, detail global)`. Songs is absent on
/// purpose: it drills into nothing, so it has no restore to wait for.
const DETAIL_TABS: [(&str, &str, &str); 4] = [
    ("tab-albums", "album-open", "AlbumDetail"),
    ("tab-artists", "artist-open", "ArtistDetail"),
    ("tab-genres", "genre-open", "GenreDetail"),
    ("tab-playlists", "playlist-open", "PlaylistDetail"),
];

/// The four seeds, each named for the failure message.
const SEEDS: [(&str, &str); 4] = [
    ("albums", include_str!("../albums/detail.rs")),
    ("artists", include_str!("../artists/detail.rs")),
    ("genres", include_str!("../genres/detail.rs")),
    ("playlists", include_str!("../playlists/detail.rs")),
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

/// The body of `seed_detail_from_settings`, or the empty string where the pin has lost it.
fn seed_body(source: &str) -> &str {
    source.split_once("pub fn seed_detail_from_settings").map_or("", |(_, body)| body)
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

/// **The raise has to be synchronous.** `seed_detail_from_settings` runs inside `install_views`,
/// well before `app.show()`, and that is the whole window it has: moved inside the spawn it lands
/// after the first paint and the flash comes back with every other line of the fix still in place.
#[test]
fn every_seed_raises_the_flag_before_it_spawns_the_fetch() {
    for (view, source) in SEEDS {
        let seed = seed_body(source);
        let raise = seed.find("set_restoring(true)").unwrap_or(usize::MAX);
        let spawn = seed.find("runtime.spawn(").unwrap_or(0);
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
    for (view, source) in SEEDS {
        assert_eq!(
            source.matches("set_restoring(false)").count(),
            1,
            "{view} must hand `restoring` back exactly once, off the end of the fetch"
        );
    }
}
