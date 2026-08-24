//! What the two converters do with a play count.
//!
//! The pair disagrees on purpose, and the disagreement is the whole content: a kept row's count is
//! this install's, where the only count a browsed row carries is the world's.

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
