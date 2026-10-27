//! Star ratings on the tag boundary: the one place a 0–5 star count becomes what a file stores,
//! and back.
//!
//! `POPM` on `ID3v2`, `RATING` in Vorbis comments and the MP4 `rate` atom all arrive as
//! [`ItemKey::Popularimeter`], un-normalized, so both shapes are ours to know. An integer `rate`
//! stays in `Ilst`'s remainder and imports unrated; Melodia writes text, so its own round trip
//! holds.
//!
//! No `FMPS_Rating`: it buys precision a whole-star strip can't show, and lofty has no mapping, so
//! reading it would take a second parse past [`crate::media::ingest::metadata::read_tags`].

use lofty::prelude::ItemKey;
use lofty::tag::{Tag, TagType};

/// The top of the scale, matching the strip `crates/melodia-ui/ui/components/star-rating.slint` draws.
pub const MAX_STARS: i32 = 5;

/// One star's worth of the 0–100 scale Vorbis comments and the MP4 `rate` atom carry.
const PERCENT_PER_STAR: i32 = 100 / MAX_STARS;

/// What Melodia stamps on a `POPM` frame it creates: the identifier Windows' own players and most
/// others key on, so the widest-read choice and the frame a Windows-rated library has replaced, not
/// duplicated.
const POPM_EMAIL: &str = "Windows Media Player 9 Series";

/// Star ratings live in 0–5, 0 being unrated. Every path into the database or a tag clamps
/// through here, so a hand-edited or out-of-range value can never reach either.
pub fn clamp_stars(stars: i32) -> i32 {
    stars.clamp(0, MAX_STARS)
}

/// The star rating `tag` carries, or `None` when it carries none.
///
/// Takes the first entry that parses: `ID3v2` keeps one item per `POPM` frame and a file may hold
/// several, written by different players.
pub fn stars_from_tag(tag: &Tag) -> Option<i32> {
    tag.get_strings(ItemKey::Popularimeter).find_map(parse_stars)
}

/// Two shapes arrive under the one key, told apart by the separator.
///
/// lofty renders a `POPM` frame, and a `RATING:<email>` key, as `email|stars|counter`, the byte
/// already mapped through that email's scale. Everything else passes through as the number the
/// file holds. A `POPM` byte of 0 means unrated, but lofty reads it as one star, so only the bare
/// forms can still say unrated.
fn parse_stars(raw: &str) -> Option<i32> {
    let raw = raw.trim();

    if let Some(stars) = raw.split('|').nth(1) {
        return stars.trim().parse().ok().filter(|s| (1..=MAX_STARS).contains(s));
    }

    // A bare number has no agreed scale: up to five is literal stars, anything above it the 0–100
    // form. Floored at one, since a file that stored a rating isn't saying unrated.
    let value = raw.parse::<i32>().ok().filter(|v| *v >= 0)?;
    let stars = match value {
        0 => return None,
        1..=MAX_STARS => value,
        percent => (percent.min(100) + PERCENT_PER_STAR / 2) / PERCENT_PER_STAR,
    };
    Some(stars.clamp(1, MAX_STARS))
}

/// Writes `stars` in the shape the tag's format wants, 0 clearing it, and answers false only for a
/// tag type with no rating key. Collapses a file's ratings to one, so a fresh `POPM` can't sit
/// disagreeing beside another player's stale one.
pub fn write_stars(tag: &mut Tag, stars: i32) -> bool {
    let stars = clamp_stars(stars);
    if stars == 0 {
        clear(tag);
        return true;
    }

    // lofty encodes `ID3v2` alone, its writer converting the whole tag. Vorbis writes from a borrow
    // that never runs the merge, so a value left to lofty lands in `RATING` verbatim, and `Ilst`
    // passes text straight through: both are handed the number already scaled.
    let value = match tag.tag_type() {
        TagType::Id3v2 => format!("{POPM_EMAIL}|{stars}|0"),
        _ => (stars * PERCENT_PER_STAR).to_string(),
    };
    tag.insert_text(ItemKey::Popularimeter, value)
}

/// Remove every rating the tag carries.
pub fn clear(tag: &mut Tag) {
    tag.remove_key(ItemKey::Popularimeter);
}

#[cfg(test)]
#[path = "tests/rating_tags_tests.rs"]
mod tests;
