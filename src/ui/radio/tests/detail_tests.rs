//! What [`super::open_station_with`] owes every way into a station page.
//!
//! Three callers reach it and only one of them arrives with a whole [`super::StationRef`]. A row
//! click carries the uuid; the boot restore and the Mouse-4/5 replay have an id and nothing else,
//! `views.json` and the history entry both naming only that. So the open completes the ref off the
//! resolve, and the page the other two land on has to be the page the click builds.
//!
//! Pinned as a source read because the open takes a live `AppState`, a `RadioUi` and a Slint
//! global, and because what went wrong was never a value a test could have been handed: the
//! function asked the caller for a field the caller was under no obligation to fill.

use crate::test_support::{block_body, strip_line_comments};

const DETAIL: &str = include_str!("../detail.rs");

/// The file with its comments dropped, so prose about the fix can't satisfy a pin and a brace
/// quoted in one can't throw [`block_body`]'s count.
fn detail() -> String {
    strip_line_comments(DETAIL)
}

/// The body of `open_station_with`.
fn open_body(src: &str) -> &str {
    src.find("pub async fn open_station_with")
        .and_then(|at| src[at..].find("{\n").map(|rel| at + rel))
        .and_then(|open| block_body(src, open))
        .unwrap_or_default()
}

/// **Nothing may read the uuid before the resolve has supplied it.**
///
/// It cost the Vote pill, the whole directory refresh behind it (the vote count, the check verdict
/// and the state chip), and the seat equality a later vote is found by — on every restored page and
/// every Mouse-4/5 landing, for as long as it stood. None of the four is an error on screen; each
/// is simply a row that isn't there, which is why review kept passing it.
#[test]
fn the_open_completes_its_ref_before_anything_reads_the_uuid() {
    let src = detail();
    let body = open_body(&src);
    assert!(
        !body.is_empty(),
        "`open_station_with` moved or changed shape, so this pin reads nothing"
    );

    // `usize::MAX` rather than a bail, so a completion that has been deleted outright fails the
    // same assertion as one that has drifted below a reader.
    let completed = body.find("uuid: source.uuid()").unwrap_or(usize::MAX);

    for (at, _) in body.match_indices("station.uuid") {
        assert!(
            at > completed,
            "`open_station_with` reads `station.uuid` above the completion, so the two callers \
             that arrive with only an id get an empty one"
        );
    }
}

/// The completion is worth nothing if the seat keeps the caller's ref instead.
///
/// `refresh_from_directory` finds its seat by `open.station == station`, and the vote path builds
/// a complete ref from the row Slint holds. A seat carrying the id-only original matches neither,
/// so the count a vote was cast for never repaints.
#[test]
fn the_seat_and_the_refresh_both_take_the_completed_ref() {
    let src = detail();
    let body = open_body(&src);
    let completed = body.find("uuid: source.uuid()").unwrap_or(usize::MAX);

    for reader in ["station: station.clone()", "refresh_from_directory("] {
        let at = body.find(reader).unwrap_or(0);
        assert!(
            at > completed,
            "`{reader}` must come after the ref is completed, and must still be there to come \
             after it"
        );
    }
}

/// A hand-typed station has no directory entry, so the empty uuid it resolves to is the answer
/// rather than a gap — and the two gates that spend it have to stay the same test, or the page
/// offers a vote the directory has nothing to record.
#[test]
fn an_empty_uuid_is_what_closes_both_directory_gates() {
    let src = detail();
    assert!(
        src.contains("let votable = state.radio_enabled() && !station.uuid.is_empty();"),
        "the vote pill must gate on the uuid being present, not on the station having a row"
    );

    let refresh = src
        .find("pub(super) async fn refresh_from_directory")
        .and_then(|at| src[at..].find("{\n").map(|rel| at + rel))
        .and_then(|open| block_body(&src, open))
        .unwrap_or_default();
    assert!(
        refresh.trim_start().starts_with("if station.uuid.is_empty() {"),
        "`refresh_from_directory` must bail on an absent uuid before it asks the directory"
    );
}

/// **`views.json` may name a station only where there is a row to look it back up in** (D6).
///
/// A browsed station is a directory answer that was never written down, so its `id` is `0` and
/// the next launch has nothing to resolve. Persisted anyway, the restore either finds nothing and
/// lands on the tab root the long way round, or — since ids are reused, the table carrying no
/// `AUTOINCREMENT` — resolves onto whichever station later took that row.
///
/// The two halves are pinned separately because they fail differently: `station_has_row` is the
/// meaning of `id == 0`, and the guard is `persist_seat` remembering to ask it.
#[test]
fn only_a_station_with_a_row_is_named_for_the_next_launch() {
    assert!(!crate::ui::radio::station_has_row(0), "a browsed station has no row");
    assert!(crate::ui::radio::station_has_row(1));

    let browsed = super::StationRef {
        id: 0,
        uuid: "9cf9…".to_owned(),
    };
    let kept = super::StationRef {
        id: 7,
        uuid: String::new(),
    };
    assert!(!browsed.is_kept());
    assert!(kept.is_kept(), "a hand-typed station is kept and carries no uuid at all");
}

#[test]
fn the_seat_is_only_persisted_where_the_station_is_kept() {
    let src = detail();
    let body = src
        .find("pub fn persist_seat")
        .and_then(|at| src[at..].find("{\n").map(|rel| at + rel))
        .and_then(|open| block_body(&src, open))
        .unwrap_or_default();

    assert!(!body.is_empty(), "`persist_seat` moved or changed shape, so this pin reads nothing");
    assert!(
        body.contains("is_kept().then_some(open.station.id)"),
        "the id may only be persisted behind `is_kept()` — a browsed station's `0` names no row"
    );
}
