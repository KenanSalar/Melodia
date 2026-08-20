//! What a filter chip's pick does to the query.
//!
//! Every mistake available here still returns stations, just the wrong ones, so nothing on screen
//! reports one: a code sent where a name belongs substring-matches, and a name sent where a code
//! belongs is simply dropped by the API.

use super::*;

/// The query one pick leaves behind, starting from no filters at all.
fn picked(chip: ChipFilter, name: &str, code: &str) -> StationSearch {
    let mut search = StationSearch::default();
    apply_pick(chip, name, code, &mut search);
    search
}

/// Equality against a whole default query rather than one field, so a pick that also writes a
/// second one fails here rather than on somebody's screen.
#[test]
fn a_country_filters_by_code_and_the_rest_by_name() {
    assert_eq!(
        picked(ChipFilter::Country, "Germany", "DE"),
        StationSearch {
            country_code: "DE".to_owned(),
            ..StationSearch::default()
        }
    );
    assert_eq!(
        picked(ChipFilter::Language, "english", "en"),
        StationSearch {
            language: "english".to_owned(),
            ..StationSearch::default()
        },
        "a language carries an `iso_639`, and the endpoint has no parameter that takes it"
    );
    assert_eq!(
        picked(ChipFilter::Tag, "jazz", ""),
        StationSearch {
            tags: vec!["jazz".to_owned()],
            ..StationSearch::default()
        }
    );
    assert_eq!(
        picked(ChipFilter::Codec, "AAC", ""),
        StationSearch {
            codec: "AAC".to_owned(),
            ..StationSearch::default()
        }
    );
}

/// Clearing is the same edit with an empty value, so it has to land on the same field the set did.
#[test]
fn clearing_a_chip_empties_the_field_it_filled() {
    let cleared = [
        ChipFilter::Country,
        ChipFilter::Language,
        ChipFilter::Tag,
        ChipFilter::Codec,
        ChipFilter::BitrateMin,
    ];
    for chip in cleared {
        assert_eq!(
            picked(chip, "", ""),
            StationSearch::default(),
            "clearing {chip:?} must leave no filter behind"
        );
    }
}

#[test]
fn the_bitrate_chip_takes_its_floor_from_the_code() {
    assert_eq!(picked(ChipFilter::BitrateMin, "320 kbps", "320").bitrate_min, 320);
    // "Any bitrate" sends no number, and neither does anything malformed.
    assert_eq!(picked(ChipFilter::BitrateMin, "Any bitrate", "").bitrate_min, 0);
    assert_eq!(picked(ChipFilter::BitrateMin, "320 kbps", "320 kbps").bitrate_min, 0);
}
