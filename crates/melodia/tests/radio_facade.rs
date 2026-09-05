//! The `library::radio` facade's contract, in the parts only its source text carries.
//!
//! One setting turns internet radio off, and it is enforced in one place: `library::radio`'s
//! `directory_client` asks `ensure_enabled` before it hands out an HTTP client. That only works
//! while two things stay true, and neither is checkable from inside the crate that holds it —
//! every outbound call in the facade takes its client from behind the guard, and nothing outside
//! the facade reaches the directory client at all.
//!
//! **The two halves are one property and they live together.** Left in their own crates they
//! ended up in `melodia-app` and `melodia-net`, each seeing half: the first passes with a second
//! module fetching on its own, the second passes with every call in the facade on a raw client.
//!
//! Source walks because the property is about every call site at once. `TestServer::requests()`
//! can now say a particular call did not happen, which is one call site per test and exactly the
//! coverage a new one added off the facade would sit outside. The play-count ordering below rides
//! along on the same corpus: driving it needs an `AppState` and a station that is reliably down,
//! where the ordering is the whole invariant and is legible from the text.

use melodia_testkit::{rust_sources, stripped_sources};

/// The facade's own tree, from the repo root.
const FACADE_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-app/src/library/radio");

/// Vacuity floor for [`facade_source`], loose enough that folding two submodules together does
/// not trip it and tight enough that a walk reading nothing cannot pass the pins standing on it.
const MIN_FACADE_FILES: usize = 4;

/// Every file the facade is made of, concatenated, with line comments stripped.
///
/// **Read off the directory rather than named**, which is what makes the walks below cover a
/// submodule nobody has written yet. The facade was one file when they were written and a split
/// that re-anchored them onto `mod.rs` alone would have left four fifths of it unmeasured — a
/// refactor that looks like an improvement and quietly disables a check. Through the shared
/// walker, which recurses: a `read_dir` answers that claim with the top of the directory alone.
fn facade_source() -> String {
    let mut source = String::new();
    for (_, text) in stripped_sources(FACADE_DIR, "rs", MIN_FACADE_FILES) {
        source.push_str(&text);
        source.push('\n');
    }
    source
}

/// **"Off" means no traffic, and this file is the only place that can be true.**
///
/// D15's switch is enforced at the facade rather than at the sidebar row, because a row that
/// disappears stops nothing a stale callback or an in-flight fetch has already started. What makes
/// one guard enough is that every outbound call reaches its client through
/// [`super::directory_client`], which is [`super::ensure_enabled`] plus the handle — so the check
/// is unskippable rather than remembered per call site.
///
/// [`only_the_radio_facade_reaches_the_directory_client`] holds the
/// other direction, that nothing *outside* this module reaches the directory at all. Neither test
/// covers the other's half: that one would pass with every call here on a raw client, and this one
/// would pass with a second module fetching on its own.
///
/// A source walk because a per-call-site assertion is exactly what a new call added off the
/// facade would not be covered by.
#[test]
fn every_outbound_call_takes_its_client_from_behind_the_switch() {
    let src = facade_source();

    // Receiver-agnostic: counting `state.http_client()` would leave a reach spelled off any other
    // binding uncounted, which is the one thing this test is for.
    let handles = src.matches(".http_client()").count();
    assert_eq!(
        handles, 1,
        "`http_client()` may be named exactly once in `library::radio`, inside `directory_client` \
         — every other reach past the guard is traffic a user who switched Radio off still pays"
    );

    let seam = src
        .split_once("fn directory_client")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);
    assert!(!seam.is_empty(), "`directory_client` moved or changed shape");
    assert!(
        seam.contains("ensure_enabled(state)?"),
        "the seam is only a seam while it asks `ensure_enabled` first"
    );
    assert!(
        seam.contains("http_client()"),
        "the one `http_client()` this test counts must be the one inside the seam"
    );
}

/// Where this module may be named, relative to its crate root: its own declaration.
///
/// An allowlist rather than the per-file *counts* `tests/binary_path.rs` pins `current_exe` with.
/// There a second call in an exempt file is itself the regression; here the facade is meant to grow
/// one per surface it gains.
const CALLER_DECL: &str = "services/net/mod.rs";

/// The facade, as a prefix rather than a file list.
///
/// It is a directory now and three of its files reach the directory client. Listing them would
/// cost an edit per submodule, and a listed name that moves pre-authorises whatever takes its
/// path next — [`OWN_TREE`]'s argument, from the other side of the same wall.
const CALLER_TREE: &str = "library/radio/";

/// This module's own tree. A prefix rather than a file list, so a fourth source
/// beside the three needs no edit.
const OWN_TREE: &str = "services/net/radio_browser/";

/// The module doc's "nothing outside `library::radio` should reach here" is what
/// leaves the setting that turns radio off one place to guard rather than one
/// per call site. It is violable from any file, so a walk holds it rather than
/// review.
///
/// The needle is the module name itself rather than the `services::net::radio_browser`
/// path, which a sibling under `services/net/` could dodge with a `super::` import.
/// Two seams it shares with the tree's other corpus pins: `strip_line_comments`
/// handles `//` and not `/* */`, and the match is a substring rather than a parse.
#[test]
fn only_the_radio_facade_reaches_the_directory_client() {
    const NEEDLE: &str = "radio_browser";

    let mut reaching = Vec::new();
    let mut declaration_seen = false;
    let mut facade_files = 0usize;

    for (path, src) in rust_sources() {
        if path.starts_with(OWN_TREE) || !src.contains(NEEDLE) {
            continue;
        }
        if path == CALLER_DECL {
            declaration_seen = true;
        } else if path.starts_with(CALLER_TREE) {
            facade_files += 1;
        } else {
            reaching.push(path);
        }
    }

    assert!(
        reaching.is_empty(),
        "{reaching:?} name `{NEEDLE}` directly. Go through `library::radio`, which is where \
         the setting that turns radio off is enforced"
    );
    assert!(
        declaration_seen,
        "`{CALLER_DECL}` no longer names `{NEEDLE}`, so a moved declaration has pre-authorised \
         whatever takes its path next"
    );
    assert!(
        facade_files > 0,
        "no file under `{CALLER_TREE}` names `{NEEDLE}`, so the facade has stopped being the door \
         and this walk is passing over an empty set"
    );
}

/// The count records that the user *chose* a station, so it must not be conditional on the server
/// being up — and the natural spelling, `player_play_station(..).await?` ahead of `mark_played`,
/// makes it exactly that. Pinned by reading the source because the alternative needs an
/// `AppState`, a socket and a station that is reliably down; the ordering is the whole invariant
/// and it is legible from the text.
#[test]
fn a_station_that_cannot_be_reached_is_still_counted_as_played() {
    let source = facade_source();
    let body = source
        .split_once("pub async fn play_station")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);

    assert!(!body.is_empty(), "`play_station` moved or changed shape, so this pin reads nothing");
    assert!(
        matches!(
            (body.find("mark_played"), body.find("player_play_station")),
            (Some(counted), Some(opened)) if counted < opened
        ),
        "`play_station` must count the play before it opens the stream, or a station that is down \
         today never reaches the recents list that would let the user find it again"
    );
}

/// **One switch, one reading of it.** `ensure_enabled` is where "off means no traffic" is decided,
/// and a second door reading `state.radio_enabled` for itself is a copy that can be got wrong
/// separately — `station_to_restore` spelled one by hand until this walk was written.
///
/// Fails in both directions, which is the point of an equality: a second reader takes the count to
/// two, and deleting the one that enforces it takes the count to zero.
#[test]
fn the_switch_is_read_in_one_place() {
    let src = facade_source();

    assert_eq!(
        src.matches("radio_enabled").count(),
        1,
        "`radio_enabled` may be named exactly once in `library::radio`, inside `ensure_enabled` — \
         a door that reads it for itself is a guard nothing else can hold"
    );
}

/// **The stream is the reach the client count cannot see.** A station opens through
/// `PlaybackContext.http` rather than through [`super::directory_client`], so
/// [`every_outbound_call_takes_its_client_from_behind_the_switch`] passes with `play_station`'s
/// guard deleted and a user who switched Radio off still streaming — from a restored queue, a
/// media key, or the Now-Playing bar.
#[test]
fn a_station_reaches_the_deck_only_from_behind_the_switch() {
    let src = facade_source();

    assert_eq!(
        src.matches("playback_ctx()").count(),
        1,
        "the transport is reached from one place in the facade, or this pin covers only one of them"
    );

    let body = src
        .split_once("pub async fn play_station")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);
    assert!(!body.is_empty(), "`play_station` moved or changed shape, so this pin reads nothing");
    assert!(
        body.contains("ensure_enabled(state)?"),
        "the one door that seats a station has to ask the switch itself — its client comes off \
         `PlaybackContext`, which no count of `http_client()` reaches"
    );
}
