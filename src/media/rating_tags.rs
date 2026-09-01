//! Star ratings on the tag boundary: the one place a 0–5 star count becomes what a file stores,
//! and back.
//!
//! Every format keeps its rating somewhere different and on a different scale — `POPM` on `ID3v2`,
//! `RATING` in Vorbis comments, the `rate` atom on MP4 — and lofty funnels all three onto
//! [`ItemKey::Popularimeter`]. What it does *not* do is normalize the value, so both the shape
//! that arrives and the shape to write back are ours to know.
//!
//! The MP4 arm reads narrower than it writes: `Ilst::split_tag` only lifts `UTF8`/`UTF16` atoms
//! into the generic tag, so an integer `rate` — which is what `MusicBee` writes on M4A — stays in
//! the remainder and never reaches [`stars_from_tag`]. What Melodia writes is text, so its own
//! round trip holds; someone else's integer imports unrated.
//!
//! Deliberately no `FMPS_Rating`, the one carrier with a spec'd scale. It buys precision a
//! whole-star strip can't show, and lofty has no mapping for it at all: an `ID3v2`
//! `TXXX:FMPS_Rating` lands in a `pub(crate)` companion tag reachable only through a second
//! parse, where [`crate::media::metadata::read_tags`] is the tree's only lofty open. The cost is
//! an Amarok or Clementine library importing unrated.

use lofty::prelude::ItemKey;
use lofty::tag::{Tag, TagType};

/// The top of the scale, matching the strip `melodia-ui/ui/components/star-rating.slint` draws.
pub const MAX_STARS: i32 = 5;

/// One star's worth of the 0–100 scale Vorbis comments and the MP4 `rate` atom carry.
const PERCENT_PER_STAR: i32 = 100 / MAX_STARS;

/// What Melodia stamps on a `POPM` frame it creates.
///
/// Windows Explorer, WMP, `MusicBee`, `foobar2000` and Winamp all key on this identifier, and its
/// byte table is the one Explorer's read ranges are drawn for — so it is both the widest-read
/// choice and, on a library already rated under Windows, the frame that gets replaced rather
/// than duplicated.
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
/// lofty renders a `POPM` frame — and a `RATING:<email>` Vorbis key — as `email|stars|counter`,
/// having already run the raw byte through the provider that email names. Picard's
/// 51/102/153/204/255 and WMP's 1/64/128/196/255 therefore both land as stars, with no scale
/// left to guess. Everything else (a bare Vorbis `RATING`, MP4's `rate`) passes through as the
/// number the file holds.
///
/// A `POPM` byte of 0 means unrated, but lofty maps it to one star and drops the byte before we
/// see it, so it reads as one star here. Only the bare forms can still say unrated.
fn parse_stars(raw: &str) -> Option<i32> {
    let raw = raw.trim();

    if let Some(stars) = raw.split('|').nth(1) {
        return stars.trim().parse().ok().filter(|s| (1..=MAX_STARS).contains(s));
    }

    // A bare number has no agreed scale. Values inside the strip's width are `foobar2000`'s
    // literal stars; anything above it is the 0–100 form MusicBee and MP4 write. Rounded to the
    // nearest star and floored at one, since a file that bothered to store a rating is not
    // saying unrated.
    let value = raw.parse::<i32>().ok().filter(|v| *v >= 0)?;
    let stars = match value {
        0 => return None,
        1..=MAX_STARS => value,
        percent => (percent.min(100) + PERCENT_PER_STAR / 2) / PERCENT_PER_STAR,
    };
    Some(stars.clamp(1, MAX_STARS))
}

/// Write `stars` into `tag` in the shape its format wants; 0 clears the rating.
///
/// Collapses a file's ratings to one — [`Tag::insert_text`] replaces every item under the key —
/// which is the point: a fresh `POPM` sitting beside the stale one another player wrote leaves
/// the two disagreeing, and which a reader believes is its own business.
///
/// False only if the tag type has no rating key at all, which none of the three primary types
/// Melodia writes is.
pub fn write_stars(tag: &mut Tag, stars: i32) -> bool {
    let stars = clamp_stars(stars);
    if stars == 0 {
        clear(tag);
        return true;
    }

    // `ID3v2` is the one type lofty encodes for us: its writer converts the whole tag, so an
    // `email|stars|counter` gets looked up and written as that provider's byte. Every other type
    // is handed the number already scaled, and Vorbis is the reason the rule is phrased that way
    // rather than per-format — `ogg::tag::create_vorbis_comments_ref` writes from a *borrow* of
    // the tag and never runs the `merge_tag` that would map it, so a value lofty was trusted to
    // encode lands in `RATING` verbatim. `Ilst` reaches the `rate` atom the same way by a
    // different route, its merge passing text straight through.
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
