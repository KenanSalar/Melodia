//! Rating-tag tests: the two shapes that arrive under one key, and the round trip back out.
//!
//! Nothing here touches a file. The module's whole job is deciding what a string means and what
//! to write in its place, and both halves are pure — the parse is what a foreign library's
//! ratings survive on, and the write is what makes Melodia's own survive a rescan.

use lofty::prelude::ItemKey;
use lofty::tag::{Tag, TagType};

use super::*;

/// A tag carrying one raw rating string, for the parse cases.
fn tag_rated(tag_type: TagType, raw: &str) -> Tag {
    let mut tag = Tag::new(tag_type);
    tag.insert_text(ItemKey::Popularimeter, raw.to_owned());
    tag
}

fn stars(raw: &str) -> Option<i32> {
    stars_from_tag(&tag_rated(TagType::VorbisComments, raw))
}

fn rating_text(tag: &Tag) -> Option<&str> {
    tag.get_string(ItemKey::Popularimeter)
}

// ------------------------------------------------------------------ the pipe form

/// lofty has already run the raw `POPM` byte through the provider its email names, so the middle
/// field is stars outright — Picard's 51/102/153/204/255 and WMP's 1/64/128/196/255 both land
/// here with no scale left to guess.
#[test]
fn the_middle_field_of_a_popm_triple_is_taken_as_stars() {
    for want in 1..=MAX_STARS {
        assert_eq!(
            stars(&format!("Windows Media Player 9 Series|{want}|0")),
            Some(want),
            "a POPM triple states its stars in the middle field"
        );
    }
}

/// A zero there is lofty's "no rating" and the strip has no way to draw it as one, so it is not a
/// star count — the `1..=MAX_STARS` filter is what says so.
#[test]
fn a_popm_triple_outside_the_strips_range_is_no_rating_at_all() {
    for raw in [
        "someone@example.org|0|0",
        "someone@example.org|6|0",
        "x|-1|0",
    ] {
        assert_eq!(stars(raw), None, "{raw} names no star the strip can draw");
    }
}

/// The pipe branch `return`s rather than falling through, and that is deliberate: a triple whose
/// middle field is junk is a malformed `POPM`, not a bare number to reinterpret on some other
/// scale. Falling through would read `"x|y|60"` as three stars off a field that never meant one.
#[test]
fn a_malformed_triple_does_not_fall_through_to_the_bare_scale() {
    assert_eq!(stars("x|not-a-number|60"), None);
    assert_eq!(stars("x||60"), None);
}

#[test]
fn surrounding_whitespace_is_ignored_on_both_shapes() {
    assert_eq!(stars("  player | 4 | 0  "), Some(4));
    assert_eq!(stars("  80  "), Some(4));
}

// ------------------------------------------------------------------ the bare form

/// `foobar2000` writes the star count itself, so anything inside the strip's own width is taken
/// literally rather than as a percentage of it.
#[test]
fn a_bare_number_inside_the_strips_width_is_literal_stars() {
    for want in 1..=MAX_STARS {
        assert_eq!(stars(&want.to_string()), Some(want));
    }
}

/// Above the strip's width there is only one scale it could be: the 0–100 form `MusicBee` and the
/// MP4 `rate` atom write. Each band rounds to its nearest star.
#[test]
fn a_bare_number_above_the_strips_width_is_read_as_the_hundred_point_scale() {
    for (percent, want) in [(20, 1), (40, 2), (60, 3), (80, 4), (100, 5)] {
        assert_eq!(stars(&percent.to_string()), Some(want), "{percent}% is {want} stars");
    }
    // Halfway between two bands rounds up, which is what the `+ PERCENT_PER_STAR / 2` is for.
    for (percent, want) in [(30, 2), (50, 3), (70, 4), (90, 5)] {
        assert_eq!(stars(&percent.to_string()), Some(want), "{percent}% rounds up to {want}");
    }
}

/// Over-scale values are clamped rather than refused: a file that bothered to store 255 is saying
/// full marks, not saying nothing.
#[test]
fn a_bare_number_past_the_top_of_the_scale_saturates() {
    assert_eq!(stars("100"), Some(MAX_STARS));
    assert_eq!(stars("255"), Some(MAX_STARS));
    assert_eq!(stars("9999"), Some(MAX_STARS));
}

/// Between the literal band and one full star's worth of percent there is nothing a rounding can
/// reach, and a stored rating is never "unrated" — so the floor is one star rather than zero.
#[test]
fn a_bare_number_under_one_stars_worth_of_percent_still_reads_as_one_star() {
    assert_eq!(stars("6"), Some(1));
    assert_eq!(stars("10"), Some(1));
}

/// The one value that means unrated, and the only shape that can still say it — a `POPM` byte of
/// zero never reaches here, lofty having mapped it to one star already.
#[test]
fn a_bare_zero_is_the_one_way_a_tag_says_unrated() {
    assert_eq!(stars("0"), None);
}

#[test]
fn a_rating_that_is_not_a_number_is_no_rating() {
    for raw in ["", "   ", "four", "3.5", "-20", "0x40"] {
        assert_eq!(stars(raw), None, "{raw:?} names no rating");
    }
}

/// `ID3v2` keeps one item per `POPM` frame and a file may hold several, written by different
/// players. The first that parses wins, so a junk frame ahead of a good one costs nothing.
#[test]
fn the_first_entry_that_parses_is_the_one_taken() {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.push(lofty::tag::TagItem::new(
        ItemKey::Popularimeter,
        lofty::tag::ItemValue::Text("not-a-rating".to_owned()),
    ));
    tag.push(lofty::tag::TagItem::new(
        ItemKey::Popularimeter,
        lofty::tag::ItemValue::Text("some player|4|0".to_owned()),
    ));

    assert_eq!(stars_from_tag(&tag), Some(4));
}

#[test]
fn a_tag_with_no_rating_key_carries_no_stars() {
    assert_eq!(stars_from_tag(&Tag::new(TagType::VorbisComments)), None);
}

// ------------------------------------------------------------------ writing

/// `ID3v2` is the one type lofty encodes for us, so it is handed the triple and left to look the
/// provider up. Everything else is handed the number already scaled.
#[test]
fn id3v2_is_written_as_a_popm_triple_and_everything_else_as_a_percentage() {
    let mut id3 = Tag::new(TagType::Id3v2);
    assert!(write_stars(&mut id3, 4));
    assert_eq!(rating_text(&id3), Some("Windows Media Player 9 Series|4|0"));

    for tag_type in [TagType::VorbisComments, TagType::Mp4Ilst] {
        let mut tag = Tag::new(tag_type);
        assert!(write_stars(&mut tag, 4));
        assert_eq!(rating_text(&tag), Some("80"), "{tag_type:?} takes the scaled number");
    }
}

/// The property the whole module exists for: whatever Melodia writes, Melodia reads back
/// unchanged. Both encodings, every star on the strip.
#[test]
fn every_star_survives_its_own_round_trip_on_both_encodings() {
    for tag_type in [TagType::Id3v2, TagType::VorbisComments, TagType::Mp4Ilst] {
        for want in 1..=MAX_STARS {
            let mut tag = Tag::new(tag_type);
            assert!(write_stars(&mut tag, want));
            assert_eq!(
                stars_from_tag(&tag),
                Some(want),
                "{want} star(s) did not survive a {tag_type:?} round trip"
            );
        }
    }
}

/// Zero is a clear, not a stored zero — a `RATING` of `"0"` would read back as unrated anyway,
/// but a ghost key left behind is one another player can still find and disagree about.
#[test]
fn writing_zero_removes_the_key_rather_than_storing_a_zero() {
    let mut tag = tag_rated(TagType::VorbisComments, "80");
    assert!(write_stars(&mut tag, 0));

    assert_eq!(rating_text(&tag), None);
    assert_eq!(stars_from_tag(&tag), None);
}

/// Out-of-range input is clamped at the boundary rather than refused, so nothing downstream has
/// to re-check what it hands over.
#[test]
fn an_out_of_range_star_count_is_clamped_before_it_is_written() {
    let mut over = Tag::new(TagType::Id3v2);
    assert!(write_stars(&mut over, 99));
    assert_eq!(stars_from_tag(&over), Some(MAX_STARS));

    // Negative collapses onto the clear, which is what `clamp_stars` makes of it.
    let mut under = tag_rated(TagType::Id3v2, "some player|3|0");
    assert!(write_stars(&mut under, -7));
    assert_eq!(rating_text(&under), None);
}

/// A fresh rating replaces every one the file carried rather than sitting beside them — two
/// `POPM` frames disagreeing is exactly the state a re-rate must not leave behind.
#[test]
fn writing_collapses_every_rating_the_tag_already_carried() {
    let mut tag = Tag::new(TagType::Id3v2);
    for raw in ["one player|2|0", "another player|5|0"] {
        tag.push(lofty::tag::TagItem::new(
            ItemKey::Popularimeter,
            lofty::tag::ItemValue::Text(raw.to_owned()),
        ));
    }

    assert!(write_stars(&mut tag, 1));

    assert_eq!(tag.get_strings(ItemKey::Popularimeter).count(), 1);
    assert_eq!(stars_from_tag(&tag), Some(1));
}

#[test]
fn clear_removes_a_rating_and_is_a_no_op_without_one() {
    let mut rated = tag_rated(TagType::VorbisComments, "60");
    clear(&mut rated);
    assert_eq!(stars_from_tag(&rated), None);

    let mut unrated = Tag::new(TagType::VorbisComments);
    clear(&mut unrated);
    assert_eq!(stars_from_tag(&unrated), None);
}

#[test]
fn clamp_stars_bounds_both_ends_of_the_strip() {
    assert_eq!(clamp_stars(-1), 0);
    assert_eq!(clamp_stars(0), 0);
    assert_eq!(clamp_stars(MAX_STARS), MAX_STARS);
    assert_eq!(clamp_stars(MAX_STARS + 1), MAX_STARS);
}

/// [`MAX_STARS`] is the scale every conversion here divides by, and the strip that draws it spells
/// its own count as a loop bound. Nothing else holds the two together, so a sixth star added to
/// the markup would silently leave every percentage reading one star short.
#[test]
fn the_strip_draws_exactly_max_stars_glyphs() {
    const STRIP: &str =
        include_str!("../../../../../../melodia-ui/ui/components/star-rating.slint");

    let needle = format!("for i in {MAX_STARS}:");
    assert!(
        crate::test_support::strip_line_comments(STRIP).contains(&needle),
        "`star-rating.slint` no longer draws {MAX_STARS} glyphs — `{needle}` is gone, so the \
         markup and `MAX_STARS` have parted company"
    );
}
