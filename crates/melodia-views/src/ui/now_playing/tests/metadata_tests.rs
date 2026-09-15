//! The technical chip row: what an absent field renders as, and which chips a row shows.

use super::{to_slint_track_meta, visible_chip_texts};
use melodia_core::entities::track::TrackMeta;

fn meta() -> TrackMeta {
    TrackMeta {
        id: 7,
        codec: Some("flac".to_owned()),
        bitrate: Some(1024),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        channels: Some(2),
        year: Some(1959),
        genre: Some("Jazz".to_owned()),
    }
}

fn chips(t: &TrackMeta) -> Vec<String> {
    visible_chip_texts(&to_slint_track_meta(t)).iter().map(ToString::to_string).collect()
}

#[test]
fn a_full_row_renders_every_chip_in_the_order_the_view_declares_them() {
    assert_eq!(
        chips(&meta()),
        ["FLAC", "1024 kbps", "44.1 kHz", "16-bit", "Stereo", "1959", "Jazz"]
    );
}

/// **Empty, not absent.** The view gates each chip on `field != ""`, so an `Option` has to arrive
/// as a string it can compare — a row is a Slint struct with no nullable slot.
#[test]
fn a_field_the_track_has_nothing_for_renders_as_an_empty_string() {
    let bare = TrackMeta {
        id: 7,
        codec: None,
        bitrate: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        year: None,
        genre: None,
    };
    let row = to_slint_track_meta(&bare);

    assert_eq!(row.codec.as_str(), "");
    assert_eq!(row.bitrate.as_str(), "");
    assert_eq!(row.year.as_str(), "");
    assert!(visible_chip_texts(&row).is_empty());
}

/// A year of zero is a tag that said nothing rather than a release in year nought, and a chip
/// stating it reads as a fact the file never carried.
#[test]
fn a_year_of_zero_is_not_a_year() {
    let unyeared = TrackMeta { year: Some(0), ..meta() };

    assert_eq!(to_slint_track_meta(&unyeared).year.as_str(), "");
}

#[test]
fn only_the_fields_a_track_carries_reach_the_strip() {
    let partial = TrackMeta { bitrate: None, bit_depth: None, genre: None, ..meta() };

    assert_eq!(chips(&partial), ["FLAC", "44.1 kHz", "Stereo", "1959"]);
}

/// The codec is uppercased here rather than at the chip, so the Summary tab and the strip cannot
/// come to spell it differently.
#[test]
fn the_codec_is_uppercased_once_on_the_way_in() {
    assert_eq!(to_slint_track_meta(&meta()).codec.as_str(), "FLAC");
}
