use crate::entities::radio::{DirectoryStation, StationPage};

use super::hide_hls;

/// One directory row, segmented or not. Spelled out rather than defaulted: `DirectoryStation` has
/// no `Default`, a station with no uuid and no URL being one nothing may keep.
fn station(name: &str, hls: bool) -> DirectoryStation {
    DirectoryStation {
        station_uuid: format!("uuid-{name}"),
        name: name.to_owned(),
        stream_url: format!("https://example.test/{name}"),
        homepage: None,
        favicon_url: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        state: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls,
        votes: 0,
        click_count: 0,
        last_check_ok: true,
    }
}

fn mixed_page() -> StationPage {
    StationPage {
        stations: vec![station("a", false), station("b", true), station("c", false)],
        has_more: true,
    }
}

#[test]
fn hiding_segmented_stations_drops_them_and_keeps_the_rest() {
    let mut page = mixed_page();
    hide_hls(&mut page, true);

    let kept: Vec<&str> = page.stations.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(kept, ["a", "c"]);
}

/// The directory served and counted the rows this drops, so paging has to step over them. Rewound
/// here, a query whose answers are mostly HLS stops after its first page.
#[test]
fn hiding_segmented_stations_leaves_the_paging_flag_alone() {
    let mut page = mixed_page();
    hide_hls(&mut page, true);
    assert!(page.has_more, "the drop is the client's, and `has_more` is the directory's answer");

    let mut ended = StationPage {
        has_more: false,
        ..mixed_page()
    };
    hide_hls(&mut ended, true);
    assert!(!ended.has_more, "and it must not be invented either");
}

#[test]
fn leaving_them_shown_changes_nothing() {
    let mut page = mixed_page();
    hide_hls(&mut page, false);
    assert_eq!(page, mixed_page());
}

/// The count records that the user *chose* a station, so it must not be conditional on the server
/// being up — and the natural spelling, `player_play_station(..).await?` ahead of `mark_played`,
/// makes it exactly that. Pinned by reading the source because the alternative needs an
/// `AppState`, a socket and a station that is reliably down; the ordering is the whole invariant
/// and it is legible from the text.
#[test]
fn a_station_that_cannot_be_reached_is_still_counted_as_played() {
    let source = include_str!("../radio.rs");
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
