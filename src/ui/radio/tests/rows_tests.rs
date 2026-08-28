//! What the two converters do with a play count, and what [`patch_grid`] refuses.
//!
//! The converters disagree on purpose: a kept row's count is this install's, where the only count
//! a browsed row carries is the world's.
//!
//! **What `patch_grid` owes a test is the refusals.** Writing a moved field onto a mounted card is
//! the visible half; the half that costs something when it goes wrong is that every way the grid
//! has stopped being the same stations in the same places has to come back `false`, a wrong `true`
//! leaving the page drawing one station's fields under another's name. The value comparison behind
//! the write is not pinned here and can't be: observing a skipped `set_row_data` means counting
//! notifications, and `slint` re-exports neither `ModelChangeListener` nor any way to build a
//! `ModelPeer`.

use std::rc::Rc;

use slint::ModelRc;

use crate::ui::grid_rows::chunk_built_rows;

use super::*;

/// A kept station with a play history behind it.
fn kept(plays: i32) -> RadioStation {
    RadioStation {
        id: 7,
        station_uuid: Some("uuid-7".to_owned()),
        name: "Test Station".to_owned(),
        stream_url: "https://example.test/stream".to_owned(),
        homepage: None,
        local_homepage: None,
        favicon_url: None,
        local_favicon_url: None,
        local_tags: None,
        local_country: None,
        artwork_path: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        is_favorite: false,
        sort_key: "test station".to_owned(),
        date_added: "2026-01-01T00:00:00.000+00:00".to_owned(),
        last_played: Some("2026-02-01T00:00:00.000+00:00".to_owned()),
        play_count: plays,
    }
}

/// One directory row, carrying the popularity figure the table does not keep.
fn browsed(click_count: i64) -> DirectoryStation {
    DirectoryStation {
        station_uuid: "uuid-9".to_owned(),
        name: "Test Station".to_owned(),
        stream_url: "https://example.test/stream".to_owned(),
        homepage: None,
        favicon_url: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        state: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        votes: 0,
        click_count,
        last_check_ok: true,
    }
}

/// A kept row naming one station, built off [`kept`] so the row's fields stay spelled in one
/// place. `uuid` is `None` for a station the user typed in.
fn kept_row(id: i64, uuid: Option<&str>) -> RadioStationRow {
    let mut station = kept(0);
    station.id = id;
    station.station_uuid = uuid.map(str::to_owned);
    to_slint_kept_station_row(&station)
}

/// One directory row, which carries a uuid and never an id.
fn browsed_row(uuid: &str) -> RadioStationRow {
    let mut station = browsed(0);
    uuid.clone_into(&mut station.station_uuid);
    to_slint_radio_station_row(&station, false, None)
}

/// The model shape both grids install. Through the caller's own chunker, so a change to how a
/// grid is laid out reaches these tests rather than passing beside them.
fn grid_of(cards: Vec<RadioStationRow>, columns: i32) -> ModelRc<RadioStationGridRow> {
    let rows = chunk_built_rows(cards, columns, |stations| RadioStationGridRow { stations });
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// One chunk row's cards, as the handle a caller held *before* a patch.
fn mounted_chunk(grid: &ModelRc<RadioStationGridRow>, chunk: usize) -> ModelRc<RadioStationRow> {
    let Some(row) = grid.row_data(chunk) else {
        unreachable!("the grid under test was built with this chunk");
    };
    row.stations
}

#[test]
fn a_kept_station_carries_its_own_play_count() {
    assert_eq!(to_slint_kept_station_row(&kept(12)).play_count, 12);
}

/// A browsed row reports no plays whatever the directory says about it. `DirectoryStation` carries
/// `click_count`, which is every listener's and not the user's, and it sits one plausible edit away
/// from filling the slot that looks empty here — a Browse grid of stations the user has never heard
/// badged with four-figure counts, reading as their own history.
#[test]
fn a_browsed_station_reports_no_plays_of_its_own() {
    assert_eq!(to_slint_radio_station_row(&browsed(4_213), false, None).play_count, 0);
}

/// Every shape a segmented station arrives in with nothing to call its format, and the two it does
/// not. A blank codec is the fragmented-MP4 case: there is no elementary stream this end can name,
/// so the card would otherwise draw an empty slot where the chip beside it reads `HLS`.
#[test]
fn a_segmented_station_with_no_format_of_its_own_is_drawn_as_hls() {
    assert_eq!(display_codec("", true), facets::SEGMENTED_CODEC_LABEL);
    assert_eq!(display_codec("UNKNOWN", true), facets::SEGMENTED_CODEC_LABEL);
    assert_eq!(display_codec("unknown,H.264", true), facets::SEGMENTED_CODEC_LABEL);

    assert_eq!(display_codec("", false), "", "a direct mount that named nothing says nothing");
    assert_eq!(
        display_codec("AAC", true),
        "AAC",
        "the bucket is generalised on the chip, never over a station that named its own format"
    );
}

/// Read back through a handle taken *before* the call, which is what tells a patch from a write:
/// a `set_vec` installs a new model and leaves this one holding what it held.
#[test]
fn a_moved_field_lands_on_the_card_already_mounted() {
    let grid = grid_of(vec![kept_row(1, Some("a")), kept_row(2, Some("b"))], 2);
    let mounted = mounted_chunk(&grid, 0);

    let mut landed = kept_row(2, Some("b"));
    landed.artwork_path = SharedString::from("/logos/b.png");
    assert!(patch_grid(&grid, &[kept_row(1, Some("a")), landed], 2));

    assert_eq!(
        mounted.row_data(1).map(|card| card.artwork_path),
        Some(SharedString::from("/logos/b.png")),
        "a landed logo must reach the card the pointer is already on"
    );
}

/// **A page that lost rows is the one that bites**, and it is what a removal leaves behind. Six
/// cards at three columns chunk to two rows and so do four, so the row count agrees across the
/// change and the trailing chunk is all that disagrees: three cards mounted where one is wanted.
/// Walk only what is wanted and it is found where it already was, so the patch reports success
/// with two removed stations still drawn.
///
/// The grown and re-columned pages come back `false` through earlier guards, so they pin the
/// behaviour rather than this bail. They are here because both are ordinary paths, a load-more
/// and a window resize.
#[test]
fn a_grid_whose_shape_moved_refuses_the_patch() {
    let cards: Vec<_> = (1..=6).map(|id| kept_row(id, None)).collect();
    let grid = grid_of(cards.clone(), 3);

    let after_removal = &cards[..4];
    assert_eq!(grid.row_count(), after_removal.len().div_ceil(3), "the row count agrees");
    assert!(!patch_grid(&grid, after_removal, 3), "two removed stations would stay on screen");

    let mut after_load_more = cards.clone();
    after_load_more.push(kept_row(7, None));
    assert!(!patch_grid(&grid, &after_load_more, 3), "a landed page is a rechunk, not a patch");

    assert!(!patch_grid(&grid, &cards, 4), "a column change is a rechunk too");
}

/// Both halves of the identity, because neither carries it alone. Simplify `same_station` to
/// either one and the other kind of list patches one station's fields onto another's card, in
/// place, under the pointer. A reorder is exactly what a refetch does.
#[test]
fn a_reordered_page_refuses_the_patch_on_both_kinds_of_row() {
    let typed = grid_of(vec![kept_row(1, None), kept_row(2, None)], 2);
    assert!(
        !patch_grid(&typed, &[kept_row(2, None), kept_row(1, None)], 2),
        "hand-typed stations share an empty uuid, so only the id tells them apart"
    );

    let directory = grid_of(vec![browsed_row("a"), browsed_row("b")], 2);
    assert!(
        !patch_grid(&directory, &[browsed_row("b"), browsed_row("a")], 2),
        "browsed stations share an id of `0`, so only the uuid does"
    );
}
