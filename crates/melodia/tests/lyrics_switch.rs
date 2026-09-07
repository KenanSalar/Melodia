//! The lyrics feature's contract, in the parts only its source text carries.
//!
//! One setting decides whether a track with no words of its own is looked up, and it is read in
//! one place: `library::lyrics`' `online_lookup_enabled`. That only works while two things stay
//! true, and neither is checkable from inside the crate holding it — nothing else in the facade
//! reads the flag for itself, and nothing outside the facade reaches the directory client at all.
//!
//! **Neither half covers the other.** The first passes with a second module fetching on its own;
//! the second passes with every arm inside the facade reading the setting for itself and getting
//! one of them wrong.

use melodia_testkit::{rust_sources, stripped_sources};

/// The facade's own tree, from the repo root.
const FACADE_DIR: &str =
    concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-app/src/library/lyrics");

/// Vacuity floor for [`facade_source`], loose enough that folding two arms together does not trip
/// it and tight enough that a walk reading nothing cannot pass the pins standing on it.
const MIN_FACADE_FILES: usize = 4;

/// Every file the facade is made of, concatenated, with line comments stripped.
///
/// Read off the directory rather than named, so an arm nobody has written yet is covered the day
/// it lands. Comments go because prose about the rule reads exactly like a violation of it.
fn facade_source() -> String {
    let mut source = String::new();
    for (_, text) in stripped_sources(FACADE_DIR, "rs", MIN_FACADE_FILES) {
        source.push_str(&text);
        source.push('\n');
    }
    source
}

/// **"Off" means no traffic, and one function is where that is decided.**
///
/// An equality rather than a floor, and it fails in both directions on purpose: a second door
/// reading the setting for itself takes the count to two, and deleting the one that enforces it
/// takes the count to zero. `library::radio` carries the same pin because a hand-rolled second
/// copy is exactly what happened there.
#[test]
fn the_lyrics_switch_is_read_in_one_place() {
    let source = facade_source();

    assert_eq!(
        source.matches("lyrics_online_enabled").count(),
        1,
        "`lyrics_online_enabled` may be named exactly once in `library::lyrics`, inside \
         `online_lookup_enabled` — a second reader is a guard nothing else can hold"
    );
}

/// The one naming of the setting has to be the seam, not some other line that happens to mention
/// it. Split on the seam's own signature so the count above cannot be satisfied by a doc string.
#[test]
fn the_one_reading_of_the_switch_is_the_seam_itself() {
    let source = facade_source();
    let seam = source
        .split_once("fn online_lookup_enabled")
        .map(|(_, rest)| rest.split_once("\n}").map_or(rest, |(body, _)| body).to_owned())
        .unwrap_or_default();

    assert!(!seam.is_empty(), "`online_lookup_enabled` moved or changed shape");
    assert!(
        seam.contains("lyrics_online_enabled"),
        "the one naming this suite counts must be the one inside the seam"
    );
}

/// Where the client module is declared.
const CLIENT_DECL: &str = "services/net/mod.rs";
/// The client's own file.
const CLIENT_TREE: &str = "services/net/lrclib";
/// The one facade allowed to call it.
const CALLER_TREE: &str = "library/lyrics/";

/// **Nothing outside `library::lyrics` reaches the directory.**
///
/// The switch guards one door, so a second caller anywhere in the tree is traffic a user who
/// turned the lookup off still pays. A walk rather than a review note, because the reach costs one
/// `use` line and looks entirely reasonable at the site that adds it.
#[test]
fn only_the_lyrics_facade_reaches_the_directory_client() {
    const NEEDLE: &str = "lrclib";

    let strays: Vec<String> = rust_sources()
        .into_iter()
        .filter(|(path, src)| {
            src.contains(NEEDLE)
                && !path.starts_with(CALLER_TREE)
                && !path.starts_with(CLIENT_TREE)
                && path != CLIENT_DECL
        })
        .map(|(path, _)| path)
        .collect();

    assert!(
        strays.is_empty(),
        "{strays:?} name the lyrics directory — every lookup goes through `library::lyrics`, \
         which is the only place the switch can stop one"
    );
}

const VIEW_MENU: &str = include_str!("../../melodia-ui/ui/components/now-playing/view-menu.slint");

/// **The popup reserves its height rather than measuring it**, so a row added without moving
/// `menu-h` is drawn outside the surface. Nothing about that fails to build and nothing about it
/// looks wrong in the file: the popup is simply one row short on screen.
#[test]
fn the_view_menu_reserves_a_row_for_every_row_it_draws() {
    let rows = VIEW_MENU.matches("OverflowRow {").count();
    let reserved = format!("FlyoutMetrics.menu-row-h * {rows}");

    assert!(
        VIEW_MENU.contains(&reserved),
        "the view menu draws {rows} row(s) but `menu-h` does not reserve `{reserved}`"
    );
}

const SETTINGS_CARD: &str = include_str!("../../melodia-ui/ui/views/settings/lyrics-section.slint");
const WELCOME_CARD: &str =
    include_str!("../../melodia-ui/ui/components/onboarding/features-panel.slint");

/// **One switch, one sentence.** The Services card and the welcome card offer the same setting, and
/// the features panel says in its own header that both must describe it identically. Two copies of
/// a label drift the moment one is reworded, and the reader who met the feature on the welcome card
/// then cannot find it in Settings.
#[test]
fn the_lyrics_switch_reads_the_same_on_both_cards() {
    const LABEL: &str = r#"@tr("Look up lyrics online")"#;
    const DESCRIPTION: &str =
        r#"@tr("When a track carries no words of its own, ask lrclib.net for them")"#;

    assert!(SETTINGS_CARD.contains(LABEL), "the Settings card no longer spells the label");
    assert!(WELCOME_CARD.contains(LABEL), "the welcome card no longer spells the label");
    assert!(SETTINGS_CARD.contains(DESCRIPTION), "the Settings card reworded its description");
    assert!(WELCOME_CARD.contains(DESCRIPTION), "the welcome card reworded its description");
}
