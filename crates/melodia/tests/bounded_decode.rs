//! Every image decode in the tree runs under a dimension cap, and two files hold that.
//!
//! `image_decode::decode_capped` is the preamble almost every path takes. `cover_embed` is the one
//! documented exception — it decodes a picked cover from *memory* and needs the guessed format
//! back, which `decode_capped` does not hand over — so it opens its own reader and applies
//! `capped_limits` by hand.
//!
//! That exemption rested on review alone until this walk, and review is what it already got past:
//! it was `tag_writer`'s until the cover half moved out from under it into a new file, and every
//! suite stayed green. A third site would read as ordinary image code and cost the forged-header
//! guard that caps what a decode may allocate.

use melodia_testkit::spellings_outside;

/// The module that owns the bound, crate-`src/`-relative as the walk reports paths.
const OWNER: &str = "media/image/image_decode.rs";

/// The one file allowed to apply it by hand.
const EXEMPT: &str = "media/ingest/cover_embed.rs";

/// **An equality, not a floor.** A new site building its own `image::Limits`, or none at all,
/// compiles clean and looks like every other decode in the tree.
///
/// The counts are what stop a second call appearing *inside* a sanctioned file, which is the half
/// a file-set check walks past. Three in the owner: the two decodes it bounds and the definition
/// itself. Its third reader, `source_pixels`, takes no cap and needs none, stopping at the header
/// without decoding a pixel.
#[test]
fn the_decode_cap_is_applied_by_hand_only_where_the_shared_preamble_cannot_reach() {
    let offenders = spellings_outside("capped_limits(", &[(OWNER, 3), (EXEMPT, 1)]);

    assert!(
        offenders.is_empty(),
        "{offenders:?} apply an image decode cap by hand. Go through \
         `image_decode::decode_capped`, or argue a second exemption here"
    );
}

/// The reader is the other half of the same rule: a site that opens its own has to bound it before
/// it decodes, and one that never opens one cannot be unbounded. Reading a header is the third case
/// rather than a hole, and it is why the owner's `open` count is two.
///
/// Both spellings, because `new` reads from memory and `open` from a path and neither is a
/// substring of the other. `cover_embed` reaches only for the memory one — a second site opening a
/// *path* reader there would be `decode_capped`'s job outright.
#[test]
fn no_image_reader_is_opened_outside_the_two_files_that_bound_their_own() {
    let mut offenders = spellings_outside("ImageReader::new(", &[(OWNER, 1), (EXEMPT, 1)]);
    offenders.extend(spellings_outside("ImageReader::open(", &[(OWNER, 2)]));

    assert!(
        offenders.is_empty(),
        "{offenders:?} open an `image::ImageReader` of their own, which is a decode with whatever \
         limits `image` defaults to. Go through `image_decode`"
    );
}
