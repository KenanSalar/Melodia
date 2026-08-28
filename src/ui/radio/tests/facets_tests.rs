//! What a filter chip's pick does to the query, and what the picker it is picked from owes.
//!
//! Every mistake available in the first half still returns stations, just the wrong ones, so
//! nothing on screen reports one: a code sent where a name belongs substring-matches, and a name
//! sent where a code belongs is simply dropped by the API.
//!
//! The second half reads `facet-chip.slint`. Its three defects are all things a screenshot in
//! review shows as very nearly right — a scrollbar a pixel into the count, a column of zeroes, a
//! filter box centred in a gutter.

use super::*;

const FACET_CHIP: &str = include_str!("../../../../melodia-ui/ui/views/radio/facet-chip.slint");

/// The picker's tree, comment-stripped and flattened — its prose argues the same bindings these
/// pins read, and would answer for them.
fn picker() -> String {
    crate::test_support::normalize_ws(&crate::test_support::strip_line_comments(FACET_CHIP))
}

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
    // The codeless fallback: a caller with only a label to hand over sends it as the value.
    assert_eq!(
        picked(ChipFilter::Codec, "AAC", ""),
        StationSearch {
            codec: "AAC".to_owned(),
            ..StationSearch::default()
        }
    );
}

/// The pair a live codec row actually carries, which is the one the fallback above is not.
///
/// `drawn_as` never hands a codec row an empty `code`, so the arm the shipped path takes is the
/// other one: revert `apply_pick` to writing the name and every assertion above still passes while
/// the endpoint is sent a word it has never heard of.
#[test]
fn a_codec_pick_sends_the_directorys_own_word_rather_than_the_label() {
    assert_eq!(
        picked(ChipFilter::Codec, SEGMENTED_CODEC_LABEL, UNKNOWN_CODEC),
        StationSearch {
            codec: UNKNOWN_CODEC.to_owned(),
            ..StationSearch::default()
        }
    );
}

/// A row has to be findable by the word it draws, and by nothing it does not.
///
/// Both filter passes started out matching `facet.name` alone, which for a codec is not what is on
/// screen: the bucket the directory calls `UNKNOWN` is drawn as `HLS`, so typing the label hid the
/// row spelling it while `unknown`, a word on screen nowhere, surfaced it.
#[test]
fn a_codec_row_is_found_by_the_word_it_draws() {
    let chip = Some(ChipFilter::Codec);
    let facet = |name: &str| Facet {
        name: name.to_owned(),
        code: None,
        station_count: 1,
    };
    let found =
        |facet: &Facet, needle: &str| matches_needle(chip, facet, &row_match::fold_needle(needle));

    let rewritten = facet(UNKNOWN_CODEC);
    for needle in [SEGMENTED_CODEC_LABEL, "hls", "HL", UNKNOWN_CODEC] {
        assert!(found(&rewritten, needle), "the `HLS` row is dropped for `{needle}`");
    }
    assert!(
        equals_needle(chip, &rewritten, &row_match::fold_needle("hls")),
        "a needle spelling the whole label is exact, which is what ranks one pill over another"
    );

    // The directory writes a comma pair with no space and the label spaces it out, so neither
    // spelling is a substring of the other.
    assert!(found(&facet("UNKNOWN,H.264"), "hls"), "a comma pair is drawn as `HLS, …` and lost");

    // And only a rewritten label is tested twice: a real format gains no match it did not have.
    let plain = facet("MP3");
    assert!(found(&plain, "mp"));
    assert!(!found(&plain, "hls"), "`MP3` matched a label it never draws");
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
    // A codeless pick sends no number, and neither does anything malformed.
    assert_eq!(picked(ChipFilter::BitrateMin, "Any bitrate", "").bitrate_min, 0);
    assert_eq!(picked(ChipFilter::BitrateMin, "320 kbps", "320 kbps").bitrate_min, 0);
}

/// The option rows and the bar over them, which have to agree on one number.
///
/// `OverlayScrollbar` is declared after the list, so it hits first: parked flush against the
/// rows it paints across the count's digits and its `TouchArea` swallows the click that lands on
/// them. Both sides name `Theme.scrollbar-slot`, so a retune of the lane moves them together.
#[test]
fn the_picker_reserves_the_scrollbars_lane_beside_the_count() {
    let src = picker();
    assert!(
        src.contains("padding-right: Theme.pad-sm + Theme.scrollbar-slot;"),
        "the picker's option rows no longer reserve the bar's lane, so the count sits under it"
    );
    assert!(
        src.contains("width: Theme.scrollbar-slot;"),
        "the picker's scrollbar spells its own width rather than the lane token the rows reserve"
    );
}

/// A facet arriving with no count paints no count.
///
/// The bitrate floors are ours rather than the directory's, and no endpoint would answer how many
/// stations clear one — so those four rows arrive at zero permanently. Suppressed in the row
/// rather than at that mount, and as an `if`: a `visible: false` child still claims the row's
/// spacing, leaving the label short of the gap it just freed.
#[test]
fn a_facet_row_with_no_count_paints_no_slot() {
    let src = picker();
    assert!(
        src.contains("if root.count > 0: VerticalLayout {"),
        "the picker paints its count unconditionally — every bitrate row reads `0`"
    );
    assert!(
        !src.contains("visible: root.count"),
        "the picker hides its count rather than unmounting it, so the row keeps the gap"
    );
}

/// The popup's width, and the filter box's slot derived from it.
///
/// Both are fixed because the body scrolls, and the box is that number less the surface's own
/// padding. Spelled twice they drifted, and the box sat pinned narrow and centred in a band of
/// dead panel — so the pin is that the number appears exactly once, whatever it is retuned to.
#[test]
fn the_picker_spells_its_width_once() {
    let src = picker();
    let declared = crate::test_support::binding_value(&src, "property <length> popup-w:").trim();
    assert!(
        declared.ends_with("px"),
        "the picker no longer declares `popup-w` as the one seat of its popup's width"
    );
    assert_eq!(
        src.matches(declared).count(),
        1,
        "`{declared}` is spelled more than once in the picker — the filter box's slot has to be \
         that number less the surface's padding, and a second copy is what let the two part"
    );
    assert!(
        src.contains("width: root.popup-w;"),
        "the picker's popup no longer sizes from `popup-w`"
    );
    assert!(
        src.contains("input-width: root.popup-w - 2 * Theme.pad-xs;"),
        "the picker's filter box no longer derives its slot from the popup's own width"
    );
}
