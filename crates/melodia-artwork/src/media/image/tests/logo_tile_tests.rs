//! What a fetched station logo has to be to reach the store, and what it is filed as when it does.

use std::io::Cursor;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A square PNG of `side` px, which is what the floor measures and what the store hashes.
fn png(side: u32) -> Result<Vec<u8>, image::ImageError> {
    let mut bytes = Vec::new();
    RgbImage::from_pixel(side, side, Rgb([80, 140, 200]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)?;
    Ok(bytes)
}

/// A wide opaque source, which is the shape [`compose`] composes rather than keeping. JPEG so the
/// container the tile replaces is not the one it is stored under.
fn wordmark_jpeg(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
    let mut bytes = Vec::new();
    RgbImage::from_pixel(width, height, Rgb([20, 60, 120]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)?;
    Ok(bytes)
}

/// The floor sits at favicon size, so the smallest real logos still get in and the 1x1 that fills
/// the field in for a tracker does not.
#[test]
fn the_floor_admits_a_favicon_and_refuses_a_tracking_pixel() -> TestResult {
    let dir = tempfile::tempdir()?;

    assert!(store(&png(1)?, "png", dir.path()).is_none(), "a 1x1 must be refused");
    assert!(
        store(&png(MIN_LOGO_DIM - 1)?, "png", dir.path()).is_none(),
        "one pixel under the floor must be refused"
    );

    let stored = store(&png(MIN_LOGO_DIM)?, "png", dir.path());
    assert!(
        stored.as_ref().is_some_and(|logo| Path::new(&logo.path).exists()),
        "a {MIN_LOGO_DIM}px logo must reach the store, got {stored:?}"
    );
    // The size the answer table bills the cache for, and the one bound the row cannot re-derive.
    assert!(
        stored.is_some_and(|logo| logo.bytes > 0),
        "a stored logo has to report what it cost, or the cache has no size to hold itself to"
    );
    Ok(())
}

/// One gate at the writer rather than one per surface, so a source too small to draw leaves nothing
/// behind for a later tier to reject.
#[test]
fn a_source_under_the_floor_never_reaches_the_store() -> TestResult {
    let dir = tempfile::tempdir()?;

    let stored = store(&png(MIN_LOGO_DIM - 1)?, "png", dir.path());

    assert!(stored.is_none(), "a source under the floor is an answer with no logo in it");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0, "and it wrote nothing");
    Ok(())
}

/// A source with nothing opaque in it has no ground to build a tile from, and storing it untreated
/// is the one outcome worse than storing nothing: every tier below drops the alpha channel rather
/// than compositing it, so the card paints an empty square where its monogram was the honest
/// answer. The two "no tile" verdicts are opposite instructions and the store has to tell them
/// apart.
#[test]
fn a_source_with_no_opaque_pixel_is_refused_rather_than_stored_untreated() -> TestResult {
    let dir = tempfile::tempdir()?;
    let side = MIN_LOGO_DIM * 2;

    let mut bytes = Vec::new();
    RgbaImage::from_pixel(side, side, Rgba([200, 40, 40, 0]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)?;

    // Well clear of the floor, so the refusal is the tile's verdict and not the size guard's.
    assert!(
        store(&bytes, "png", dir.path()).is_none(),
        "a fully transparent {side}px source must not reach the store"
    );
    Ok(())
}

/// A tile is flat ground behind a hard-edged mark, which is what JPEG rings around and what PNG
/// holds in a few kilobytes. So the source's own container stops having a say the moment a tile
/// replaces it.
#[test]
fn a_composed_tile_is_stored_as_png_whatever_the_source_was() -> TestResult {
    let dir = tempfile::tempdir()?;

    let stored = store(&wordmark_jpeg(MIN_LOGO_DIM * 4, MIN_LOGO_DIM * 2)?, "jpg", dir.path());

    let Some(stored) = stored else {
        return Err("a wide opaque source is composed, not refused".into());
    };
    assert_eq!(
        Path::new(&stored.path).extension().and_then(std::ffi::OsStr::to_str),
        Some("png"),
        "a wordmark is stored as the tile it became, not the JPEG it arrived as: {}",
        stored.path
    );
    Ok(())
}

/// **The now-playing tile skips `native-size` because of this number**, so the argument for that
/// omission lives in one tree and the number it rests on lives in another.
///
/// `source-artwork.slint` reasons that no source reaching it can be small enough for
/// `ArtworkImage`'s inset arm to fire, since the floor is 32 px and the largest tile mounting it
/// is 46. Lower the floor and that stops being true with nothing to say so: the tile would upscale
/// a 16 px favicon across 46 px rather than insetting it, which is exactly the treatment the inset
/// arm exists to give. Raise it past the tile and the comment merely reads oddly.
#[test]
fn the_slint_tile_that_skips_native_size_still_agrees_with_the_floor() {
    const TILE: &str =
        include_str!("../../../../../melodia-ui/ui/components/now-playing/source-artwork.slint");

    assert!(
        TILE.contains(&format!("`media::image::logo_tile::MIN_LOGO_DIM` is {MIN_LOGO_DIM} px")),
        "`source-artwork.slint` restates the floor to argue it needs no `native-size`; it is \
         {MIN_LOGO_DIM} px here and the two have drifted"
    );
    // Stripped, or the comment making the argument satisfies the search for the binding it
    // argues against.
    assert!(
        !melodia_testkit::strip_line_comments(TILE).contains("native-size"),
        "the argument stands or the binding does — if the tile now sets `native-size`, this pin \
         and the comment above it are both stale"
    );
}
