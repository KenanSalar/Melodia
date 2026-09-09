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

use melodia_testkit::{rust_sources, strip_line_comments, stripped_sources};

/// The facade's own tree, from the repo root.
const FACADE_DIR: &str =
    concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-app/src/library/lyrics");

/// Vacuity floor for [`facade_source`], counted over the **production** files alone, loose enough
/// that folding two arms together does not trip it and tight enough that a walk reading nothing
/// cannot pass the pins standing on it.
const MIN_FACADE_FILES: usize = 5;

/// The facade's unit tests, which live under it and are not part of what these pins read.
const FACADE_TESTS: &str = "tests/";

/// Every production file the facade is made of, concatenated, with line comments stripped.
///
/// Read off the directory rather than named, so an arm nobody has written yet is covered the day
/// it lands. Comments go because prose about the rule reads exactly like a violation of it.
///
/// **`tests/` is dropped, and both pins below need it dropped.** Unlike `library::radio`, whose
/// floor this was copied from, this facade holds its unit tests inside itself: counted, four test
/// files satisfy a floor written for the seven production ones, so the whole of what the pins read
/// could be deleted under it. It also decides what the count below is a count *of* — a test naming
/// the setting is not a second reader of it.
fn facade_source() -> String {
    let mut source = String::new();
    let mut files = 0usize;

    for (path, text) in stripped_sources(FACADE_DIR, "rs", MIN_FACADE_FILES) {
        if path.starts_with(FACADE_TESTS) {
            continue;
        }
        files += 1;
        source.push_str(&text);
        source.push('\n');
    }

    assert!(
        files >= MIN_FACADE_FILES,
        "only {files} production files under `library::lyrics` — the pins below are reading a \
         facade that has mostly stopped existing"
    );
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
    // `"\n}\n"` and `map_or("")`, `library::radio`'s spelling: a bare `"\n}"` closes on `\n})` and
    // `\n};` too, and falling back to the rest of the corpus would let any naming downstream of the
    // seam satisfy the assertion below. Both make this fail open, which is the one direction a pin
    // may not fail.
    let seam = source
        .split_once("fn online_lookup_enabled")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);

    assert!(!seam.is_empty(), "`online_lookup_enabled` moved or changed shape");
    assert!(
        seam.contains("lyrics_online_enabled"),
        "the one naming this suite counts must be the one inside the seam"
    );
}

/// Where the client module is declared.
const CLIENT_DECL: &str = "services/net/mod.rs";
/// The client's own module: the requests, the providers behind them, and the matcher that judges
/// what comes back.
///
/// **The directory rather than a file list**, so a provider added beside the first is covered the
/// day it lands rather than the day someone remembers this constant. Trailing separator included,
/// or a future `lyrics_directory_v2/` exempts itself by sharing a prefix.
const CLIENT_TREE: &str = "services/net/lyrics_directory/";
/// The one facade allowed to call it.
const CALLER_TREE: &str = "library/lyrics/";

/// **Nothing outside `library::lyrics` reaches the directory.**
///
/// The switch guards one door, so a second caller anywhere in the tree is traffic a user who
/// turned the lookup off still pays. A walk rather than a review note, because the reach costs one
/// `use` line and looks entirely reasonable at the site that adds it.
///
/// The needle is the module's own name rather than any provider's: the providers are private to
/// it, so naming the module is the only way in, and no import grouping or `as` alias can spell
/// that segment differently. The one reach it cannot see is the facade re-exporting its own
/// import, which would hand the client out under a name this never reads; those imports are
/// private today. That is why the module is not simply called `lyrics`, which every file on both
/// sides of the seam contains already.
///
/// **Each of the two exemptions is asserted to still match**, `library::radio`'s shape: an
/// exemption that has stopped matching is not a spent one, it is a hole that reads as coverage —
/// a moved declaration pre-authorises whatever takes its path next, and a facade that has stopped
/// naming the client leaves this walking an empty set and passing.
#[test]
fn only_the_lyrics_facade_reaches_the_directory_client() {
    const NEEDLE: &str = "lyrics_directory";

    let mut strays = Vec::new();
    let mut declaration_seen = false;
    let mut facade_files = 0usize;

    for (path, src) in rust_sources() {
        if path.starts_with(CLIENT_TREE) || !src.contains(NEEDLE) {
            continue;
        }
        if path == CLIENT_DECL {
            declaration_seen = true;
        } else if path.starts_with(CALLER_TREE) {
            facade_files += 1;
        } else {
            strays.push(path);
        }
    }

    assert!(
        strays.is_empty(),
        "{strays:?} name the lyrics directory — every lookup goes through `library::lyrics`, \
         which is the only place the switch can stop one"
    );
    assert!(
        declaration_seen,
        "`{CLIENT_DECL}` no longer names `{NEEDLE}`, so a moved declaration has pre-authorised \
         whatever takes its path next"
    );
    assert!(
        facade_files > 0,
        "no file under `{CALLER_TREE}` names `{NEEDLE}`, so the facade has stopped being the door \
         and this walk is passing over an empty set"
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

const LYRICS_MENU: &str =
    include_str!("../../melodia-ui/ui/components/now-playing/lyrics-menu.slint");

/// **The popup reserves its height by hand, so the rows it draws have to be counted right.**
/// `menu-h` multiplies `FlyoutMetrics.menu-row-h` by a literal that nothing derives, and another
/// row added without touching it is clipped off the bottom of a popup that still looks deliberate.
///
/// The menu this one replaced carried the same pin and took it along when that file was deleted.
/// The menu came back a few commits later and the pin did not, which is the shape to watch for:
/// a pin keyed on one file's name dies with the file, and nothing says so.
#[test]
fn the_lyrics_menu_reserves_a_row_for_every_row_it_draws() {
    let source = strip_line_comments(LYRICS_MENU);
    let rows = source.matches("OverflowRow {").count();
    let reserved = format!("FlyoutMetrics.menu-row-h * {rows}");

    assert!(
        rows > 0,
        "no `OverflowRow` left in the lyrics menu — the count below would pass against nothing"
    );
    assert!(
        source.contains(&reserved),
        "the lyrics menu draws {rows} row(s) but `menu-h` does not reserve `{reserved}`"
    );
}

/// **Every row is drawn whether or not it can act.** That is what lets the reserve above be a
/// constant, and it keeps a row from moving under the pointer between one open and the next.
///
/// A row mounted behind an `if` leaves the count above correct and breaks both, so what is checked
/// is the spelling of a conditional mount: `button-enabled` gates the action, never the mount.
#[test]
fn no_lyrics_menu_row_is_mounted_behind_a_condition() {
    let source = strip_line_comments(LYRICS_MENU);

    assert!(
        !source.contains(": OverflowRow {"),
        "a lyrics menu row is mounted conditionally, so the popup's fixed reserve is now wrong \
         for one of its two states. Gate the action with `button-enabled` and draw the row either \
         way"
    );
}

/// The panel itself, whose contract is mostly what it refuses to declare.
const LYRICS_PANEL: &str =
    include_str!("../../melodia-ui/ui/components/now-playing/lyrics-panel.slint");

/// The view that decides which of three things the right-hand column is.
const NOW_PLAYING_VIEW: &str = include_str!("../../melodia-ui/ui/views/now-playing-view.slint");

/// **The panel is mounted behind an `if`, so it may declare no change tracker.**
///
/// A tracker in a dropped branch stays registered against whatever it watched, and the shape that
/// panics is one re-dirtied on the frame the branch goes. The follow is driven from a `Timer`
/// instead and the glide is arithmetic, so the file's own header states this as a rule. Nothing
/// enforced it, and the two spellings that would put it back both read as ordinary Slint.
#[test]
fn nothing_in_the_lyrics_panel_watches_a_property() {
    let source = strip_line_comments(LYRICS_PANEL);

    assert!(!source.contains("changed "), "a `changed` handler is a tracker this may not hold");
    assert!(
        !source.contains("animate "),
        "an `animate` on the scroll target is a tracker too, and the one re-dirtied every frame"
    );
    assert!(
        source.contains("Timer {"),
        "the timer is what the prohibition exists to leave in place, so its absence means this \
         pin is guarding a file that no longer follows anything"
    );
}

/// The three arms are each other's negations, and the heading is a fourth spelling of the same
/// decision. A pair that stops agreeing mounts two panels into one slot or none at all.
#[test]
fn the_now_playing_column_mounts_exactly_one_arm() {
    let source = melodia_testkit::normalize_ws(&strip_line_comments(NOW_PLAYING_VIEW));

    let arms = [
        "if !Player.vm.has_station && !Lyrics.shown: UpNextList {",
        "if !Player.vm.has_station && Lyrics.shown: LyricsPanel {",
        "if Player.vm.has_station: Rectangle {",
    ];
    for arm in arms {
        assert_eq!(
            source.matches(arm).count(),
            1,
            "the column's arms have to stay one apiece, and `{arm}` is not"
        );
    }

    // The heading names whichever arm is up, so it has to branch on the same two properties in
    // the same order. Read the other way round it labels a station panel "Lyrics".
    let station_at = source.find("Player.vm.has_station ? @tr(\"Station\")");
    let lyrics_at = source.find("Lyrics.shown ? @tr(\"Lyrics\")");
    assert!(
        matches!((station_at, lyrics_at), (Some(station), Some(lyrics)) if station < lyrics),
        "the column heading must test the station before the lyrics toggle, as the mounts do"
    );
}

/// **The bar's lane is one number and the panel spells it three times.**
///
/// The column pads itself by it, the bar is drawn at it, and the width handed to Rust has it
/// subtracted. Drop the last and every line is measured against a width the panel does not have,
/// so the wrap estimate and the drawn text disagree by exactly a scrollbar down a whole sheet.
#[test]
fn the_lyrics_panel_reserves_its_scrollbar_lane_everywhere_it_matters() {
    let source = melodia_testkit::normalize_ws(&strip_line_comments(LYRICS_PANEL));

    let roles = [
        ("the column's own padding", "padding-right: Theme.scrollbar-slot;"),
        ("the bar drawn in it", "width: Theme.scrollbar-slot;"),
        ("the width reported to Rust", "sv.visible-width - Theme.scrollbar-slot"),
    ];
    for (role, spelling) in roles {
        assert!(source.contains(spelling), "{role} no longer reads the shared lane");
    }
}
