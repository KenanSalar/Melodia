use melodia_core::error::AppError;

use super::*;

/// [`STORED_EXTENSIONS`] and the `image` dependency's feature list are one decision spelled in two
/// files, and it fails silently in both directions. A format the store may name but this build
/// cannot decode is a cover written to disk that nothing can draw — while [`is_stored_name`] still
/// claims it, so the sweep is on the hook for a file no reader will ever open.
#[test]
fn every_stored_extension_is_one_this_build_can_read() {
    for ext in STORED_EXTENSIONS {
        let readable =
            image::ImageFormat::from_extension(ext).is_some_and(|format| format.reading_enabled());
        assert!(
            readable,
            "the store may write `.{ext}` but this build cannot decode it — add the feature to \
             the `image` dependency in Cargo.toml, or take the extension out of STORED_EXTENSIONS"
        );
    }
}

/// A solid-colour square, encoded by the extension in `name`, so a sampled pixel names the source
/// it came from. Real bytes rather than a placeholder string: `store_image` reads every source's
/// header and refuses anything it could not draw, so a fixture has to be a decodable image.
fn solid_source(
    dir: &Path,
    name: &str,
    rgb: [u8; 3],
    width: u32,
    height: u32,
) -> Result<PathBuf, AppError> {
    let path = dir.join(name);
    image::RgbImage::from_pixel(width, height, image::Rgb(rgb))
        .save(&path)
        .map_err(|e| AppError::Validation(format!("write {name}: {e}")))?;
    Ok(path)
}

#[test]
fn compute_hash_returns_16_hex_chars() {
    let hash = compute_hash(b"test data");
    assert_eq!(hash.len(), 16);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn compute_hash_is_deterministic() {
    let h1 = compute_hash(b"same input");
    let h2 = compute_hash(b"same input");
    assert_eq!(h1, h2);
}

#[test]
fn compute_hash_different_inputs_differ() {
    let h1 = compute_hash(b"input A");
    let h2 = compute_hash(b"input B");
    assert_ne!(h1, h2);
}

#[test]
fn compute_hash_empty_input() {
    let hash = compute_hash(b"");
    assert_eq!(hash.len(), 16);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

// ── how the store is written ──

// ── the store's bounds ──

// `STORE_MAX_DIM` has to clear every cover tier, or each one upscales from a source the store
// already threw away. That assertion names `ui::grid_prewarm` and `ui::util`, so it cannot live
// in the tier that owns the cap: `crates/melodia/tests/cross_tier.rs` holds it from outside both.

// ── store_image ──

/// Below both bounds nothing is decoded, re-encoded or resized — the file on disk is the bytes
/// that arrived. 144 of the reference library's 227 covers take this path, so it is the common
/// case rather than the edge.
#[test]
fn a_source_inside_the_bounds_is_stored_byte_identical() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let source = solid_source(tmp.path(), "small.png", [10, 20, 30], 64, 64)?;
    let bytes = std::fs::read(&source)?;

    let stored = store_image(&bytes, "png", tmp.path())
        .ok_or_else(|| AppError::Validation("store_image returned None".into()))?;

    assert_eq!(std::fs::read(&stored)?, bytes, "an in-bounds source must not be re-encoded");
    Ok(())
}

/// Over the dimension bound the file is re-encoded, and **the name has to describe what landed**
/// — hash the source instead and the `exists()` dedup guard starts answering about bytes nobody
/// stored.
#[test]
fn an_oversized_source_is_shrunk_and_named_after_what_was_written() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    // Noise rather than a flat fill, so the JPEG actually comes out smaller than the source PNG.
    let source = tmp.path().join("big.png");
    image::RgbImage::from_fn(1024, 1024, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8])
    })
    .save(&source)
    .map_err(|e| AppError::Validation(format!("write big.png: {e}")))?;
    let bytes = std::fs::read(&source)?;

    let stored = store_image(&bytes, "png", tmp.path())
        .ok_or_else(|| AppError::Validation("store_image returned None".into()))?;
    let written = std::fs::read(&stored)?;

    let (width, height) = image_decode::memory_dimensions(&written, MAX_SOURCE_DIM)
        .ok_or_else(|| AppError::Validation("stored file will not decode".into()))?;
    assert!(
        width <= STORE_MAX_DIM && height <= STORE_MAX_DIM,
        "stored at {width}x{height}, past the cap"
    );
    assert!(written.len() < bytes.len(), "the whole point is that it got smaller");

    let name = std::path::Path::new(&stored)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::Validation("stored path has no file name".into()))?;
    assert_eq!(
        name,
        stored_name(&compute_hash(&written), "jpg"),
        "the name must describe the file"
    );
    Ok(())
}

/// The rule that makes the cap forgiving, and the real mechanism behind it: a source encoded
/// cheaply is only a little over the cap in pixels but far under it in bytes, so re-encoding it
/// at a *fixed* quality 90 costs more than the downscale saves. A cap chosen slightly too low can
/// therefore waste CPU here but can never grow the store. Live machinery rather than a
/// theoretical guard — it fires on 6 of the reference library's 83 over-cap files.
#[test]
fn a_source_that_would_grow_under_re_encode_is_left_alone() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;

    // Just past the cap, and encoded far below the quality the normalizer would use.
    let noisy = image::RgbImage::from_fn(600, 600, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8])
    });
    let mut cheap = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cheap, 20);
    image::DynamicImage::ImageRgb8(noisy)
        .write_with_encoder(encoder)
        .map_err(|e| AppError::Validation(format!("encode cheap jpeg: {e}")))?;

    let stored = store_image(&cheap, "jpg", tmp.path())
        .ok_or_else(|| AppError::Validation("store_image returned None".into()))?;

    assert_eq!(
        std::fs::read(&stored)?,
        cheap,
        "the re-encode came out larger than the source, so the source is what must be kept"
    );
    Ok(())
}

/// Header validation, which is also what stops a container this build cannot draw taking up disk
/// forever — `image` is compiled without the BMP/GIF/TIFF decoders the lofty MIME map can name.
#[test]
fn a_source_that_is_not_a_decodable_image_is_not_stored() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;

    assert!(store_image(b"", "jpg", tmp.path()).is_none(), "an empty source stores nothing");
    assert!(
        store_image(b"not an image at all", "jpg", tmp.path()).is_none(),
        "a source with no recognisable header stores nothing"
    );
    assert_eq!(std::fs::read_dir(tmp.path())?.count(), 0, "and neither leaves a file behind");
    Ok(())
}

// ── find_external_cover ──

#[test]
fn find_external_cover_found() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let album_dir = tmp.path().join("album");
    std::fs::create_dir_all(&album_dir)?;
    std::fs::write(album_dir.join("cover.jpg"), b"fake image")?;

    let cache: CoverCache = new_cover_cache();
    let track_path = album_dir.join("track.mp3");

    let cover = find_external_cover(&track_path, &cache)
        .ok_or_else(|| AppError::Validation("expected cover.jpg to be found".into()))?;
    assert!(cover.ends_with("cover.jpg"));
    Ok(())
}

#[test]
fn find_external_cover_not_found() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let cache: CoverCache = new_cover_cache();
    let track_path = tmp.path().join("track.mp3");

    let result = find_external_cover(&track_path, &cache);
    assert!(result.is_none());
    Ok(())
}

#[test]
fn find_external_cover_cache_hit() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("cover.jpg"), b"fake image")?;

    let cache: CoverCache = new_cover_cache();
    let track_path = tmp.path().join("track.mp3");

    let _ = find_external_cover(&track_path, &cache);
    let _ = find_external_cover(&track_path, &cache);

    // Only one directory entry cached
    assert_eq!(cache.dir_to_cover.lock().len(), 1);
    Ok(())
}

// ── cache_image_file ──

#[test]
fn cache_image_file_basic() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&artwork_dir)?;

    let source = solid_source(tmp.path(), "cover.jpg", [12, 34, 56], 64, 64)?;

    let cached = cache_image_file(&source, &artwork_dir)
        .ok_or_else(|| AppError::Validation("cache_image_file returned None".into()))?;
    let cached_path = std::path::Path::new(&cached);
    assert!(cached_path.exists());
    // Filename should be hash.jpg
    let name = cached_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::Validation("cached path missing file name".into()))?;
    assert!(
        std::path::Path::new(name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jpg"))
    );
    assert_eq!(name.len(), 16 + 1 + 3); // 16 hex + dot + ext
    Ok(())
}

#[test]
fn cache_image_file_dedup() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&artwork_dir)?;

    let source1 = solid_source(tmp.path(), "a.png", [7, 8, 9], 64, 64)?;
    let source2 = solid_source(tmp.path(), "b.png", [7, 8, 9], 64, 64)?;

    let r1 = cache_image_file(&source1, &artwork_dir)
        .ok_or_else(|| AppError::Validation("expected cached path 1".into()))?;
    let r2 = cache_image_file(&source2, &artwork_dir)
        .ok_or_else(|| AppError::Validation("expected cached path 2".into()))?;
    assert_eq!(r1, r2); // same content → same cached file
    Ok(())
}

#[test]
fn cache_image_file_empty() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&artwork_dir)?;

    let source = tmp.path().join("empty.jpg");
    std::fs::write(&source, b"")?;

    let result = cache_image_file(&source, &artwork_dir);
    assert!(result.is_none());
    Ok(())
}

// ── find_and_cache_artwork ──

#[test]
fn find_and_cache_artwork_external_only() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let album_dir = tmp.path().join("album");
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&album_dir)?;
    std::fs::create_dir_all(&artwork_dir)?;
    solid_source(&album_dir, "cover.jpg", [90, 90, 90], 64, 64)?;

    let cache: CoverCache = new_cover_cache();
    let track_path = album_dir.join("song.mp3");

    let result = find_and_cache_artwork(&track_path, None, &artwork_dir, &cache);
    assert!(result.is_some());
    Ok(())
}

#[test]
fn find_and_cache_artwork_memoizes_per_cover() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let album_dir = tmp.path().join("album");
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&album_dir)?;
    std::fs::create_dir_all(&artwork_dir)?;
    solid_source(&album_dir, "cover.jpg", [90, 90, 90], 64, 64)?;

    let cache: CoverCache = new_cover_cache();

    // Two tracks in the same directory resolve to the same cover file and
    // must share one memo entry (one read+hash, not one per track).
    let first = find_and_cache_artwork(&album_dir.join("a.mp3"), None, &artwork_dir, &cache)
        .ok_or_else(|| AppError::Validation("expected cached artwork path".into()))?;
    let second = find_and_cache_artwork(&album_dir.join("b.mp3"), None, &artwork_dir, &cache)
        .ok_or_else(|| AppError::Validation("expected cached artwork path".into()))?;

    assert_eq!(first, second);
    assert_eq!(cache.cover_to_cached.lock().len(), 1);
    Ok(())
}

#[test]
fn find_and_cache_artwork_no_tag_no_external() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let album_dir = tmp.path().join("empty_album");
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&album_dir)?;
    std::fs::create_dir_all(&artwork_dir)?;
    // No cover files in album_dir

    let cache: CoverCache = new_cover_cache();
    let track_path = album_dir.join("song.mp3");

    let result = find_and_cache_artwork(&track_path, None, &artwork_dir, &cache);
    assert!(result.is_none());
    Ok(())
}

#[test]
fn find_external_cover_alternate_names() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let album_dir = tmp.path().join("album");
    std::fs::create_dir_all(&album_dir)?;
    // "folder.jpg" is one of the alternate cover filenames
    std::fs::write(album_dir.join("folder.jpg"), b"album art")?;

    let cache: CoverCache = new_cover_cache();
    let track_path = album_dir.join("track.flac");

    let cover = find_external_cover(&track_path, &cache)
        .ok_or_else(|| AppError::Validation("expected folder.jpg to be found".into()))?;
    let path_str = cover.to_string_lossy().to_string();
    assert!(path_str.contains("folder.jpg"));
    Ok(())
}

#[test]
fn extract_and_cache_artwork_empty_pictures() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&artwork_dir)?;

    // Create a tag with no pictures
    let tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
    let result = extract_and_cache_artwork(&tag, &artwork_dir, &new_cover_cache());
    assert!(result.is_none());
    Ok(())
}

/// An `Id3v2` tag carrying one cover over [`STORE_MAX_DIM`], so what a memo saves is a decode and a
/// re-encode rather than a hash.
fn tag_with_oversized_cover() -> Result<lofty::tag::Tag, AppError> {
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(1024, 1024, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8])
    }))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .map_err(|e| AppError::Validation(format!("encode png: {e}")))?;

    let picture = lofty::picture::Picture::from_reader(&mut std::io::Cursor::new(&png))
        .map_err(|e| AppError::Validation(format!("read picture: {e}")))?;
    let mut tag = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
    tag.push_picture(picture);
    Ok(tag)
}

/// The store's dedup guard sits *behind* the normalizer — the stored name has to describe the
/// stored file — so every track on an album would decode and re-encode the one cover they share,
/// and throw all but one of those away. The external-cover tier gets the same saving from its path
/// key; embedded artwork has no path, hence the hash.
///
/// The second store is what makes the answer proof rather than coincidence: only a hit can name one
/// the call was never handed. Production has a single store, so it is a probe here rather than a
/// shape the caches support.
#[test]
fn an_embedded_cover_is_stored_once_however_many_tracks_carry_it() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let artwork_dir = tmp.path().join("artwork");
    let unused_dir = tmp.path().join("artwork-second");
    std::fs::create_dir_all(&artwork_dir)?;
    std::fs::create_dir_all(&unused_dir)?;

    let tag = tag_with_oversized_cover()?;
    let cache: CoverCache = new_cover_cache();
    let first = extract_and_cache_artwork(&tag, &artwork_dir, &cache)
        .ok_or_else(|| AppError::Validation("expected a stored cover".into()))?;
    let second = extract_and_cache_artwork(&tag, &unused_dir, &cache)
        .ok_or_else(|| AppError::Validation("expected a stored cover".into()))?;

    assert_eq!(second, first);
    assert!(
        std::fs::read_dir(&unused_dir)?.next().is_none(),
        "the second track re-ran the decode instead of taking the memo"
    );
    Ok(())
}

/// The sweep unlinks a stored cover once nothing references it, which can land between the memo and
/// a later track reaching it. A row written from that hit would not heal — `track_is_current` reads
/// the track as current, so nothing re-extracts.
#[test]
fn a_hit_whose_file_the_sweep_retired_is_stored_again() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir_all(&artwork_dir)?;

    let tag = tag_with_oversized_cover()?;
    let cache: CoverCache = new_cover_cache();
    let first = extract_and_cache_artwork(&tag, &artwork_dir, &cache)
        .ok_or_else(|| AppError::Validation("expected a stored cover".into()))?;

    std::fs::remove_file(&first)?;
    let second = extract_and_cache_artwork(&tag, &artwork_dir, &cache)
        .ok_or_else(|| AppError::Validation("expected a stored cover".into()))?;

    assert_eq!(second, first);
    assert!(Path::new(&second).exists(), "the memo handed back a path the sweep had unlinked");
    Ok(())
}

// ── compose_cover ──

/// The canvas side these tests compose at. `compose_cover` takes it from the caller, so a number
/// of the tests' own also pins that it is honoured rather than quietly using [`COMPOSITE_SIZE`].
const TEST_SIDE: u32 = 400;

/// The four layouts, pinned against composed pixels — the arrangement `CoverMosaic` used to draw in
/// Slint, and the collage is now the only place it is stated. The sample points are spelled here
/// rather than read off `COMPOSITE_LAYOUTS`: sampling the table the compose loop walks proves only
/// that the loop honours it, and passes just as happily when two of its rows are swapped.
#[test]
fn each_layout_puts_every_source_in_its_own_rect() -> Result<(), AppError> {
    const COLOURS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]];

    const NEAR: u32 = TEST_SIDE / 4;
    const FAR: u32 = TEST_SIDE * 3 / 4;
    const MID: u32 = TEST_SIDE / 2;
    /// Where each source's colour must land, by set size: full bleed; left | right;
    /// left | right-top over right-bottom; 2×2 read across then down.
    const SAMPLES: [&[(u32, u32)]; 4] = [
        &[(MID, MID)],
        &[(NEAR, MID), (FAR, MID)],
        &[(NEAR, MID), (FAR, NEAR), (FAR, FAR)],
        &[(NEAR, NEAR), (FAR, NEAR), (NEAR, FAR), (FAR, FAR)],
    ];

    let tmp = tempfile::tempdir()?;
    let sources = COLOURS
        .iter()
        .enumerate()
        .map(|(i, rgb)| solid_source(tmp.path(), &format!("{i}.png"), *rgb, 64, 64))
        .collect::<Result<Vec<PathBuf>, AppError>>()?;

    for (count, points) in (1..=4).zip(SAMPLES) {
        let canvas = compose_cover(&sources[..count], TEST_SIDE)
            .ok_or_else(|| AppError::Validation(format!("compose of {count} returned None")))?;
        assert_eq!((canvas.width(), canvas.height()), (TEST_SIDE, TEST_SIDE));

        for (slot, &(x, y)) in points.iter().enumerate() {
            assert_eq!(
                canvas.get_pixel(x, y).0,
                COLOURS[slot],
                "{count}-up layout, slot {slot} at ({x}, {y})"
            );
        }
    }
    Ok(())
}

/// Refused for the set's *size*, which the leniency below must not talk its way out of: dropping a
/// source is how a broken one costs its slot, never how five of them find a layout.
#[test]
fn compose_cover_refuses_a_set_it_has_no_layout_for() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let one = solid_source(tmp.path(), "one.png", [255, 0, 0], 64, 64)?;

    assert!(compose_cover(&[], TEST_SIDE).is_none());
    assert!(compose_cover(&vec![one.clone(); 5], TEST_SIDE).is_none());

    // Four readable plus a broken fifth composed a 4-up while the size check sat behind the
    // readability retry — the one arrangement of five sources that ever reached a canvas.
    let broken = tmp.path().join("broken.png");
    std::fs::write(&broken, b"not an image")?;
    let mut five = vec![one; 4];
    five.push(broken);
    assert!(
        compose_cover(&five, TEST_SIDE).is_none(),
        "a broken source must not buy a set a layout"
    );
    Ok(())
}

/// A cover that has gone missing under us costs its slot, never the banner: the layout is picked
/// from what survives, so a broken second source leaves the first full-bleed. Only an entirely
/// unreadable set reads as no artwork.
#[test]
fn an_unreadable_source_drops_out_of_the_collage() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let good = solid_source(tmp.path(), "good.png", [255, 0, 0], 64, 64)?;

    let broken = tmp.path().join("broken.png");
    std::fs::write(&broken, b"not an image")?;

    let canvas = compose_cover(&[good, broken.clone()], TEST_SIDE)
        .ok_or_else(|| AppError::Validation("a readable source must still compose".into()))?;
    // The 1-up layout rather than the 2-up: the survivor takes the half the broken source would
    // have had, which is what separates this from painting a blank quarter.
    for x in [TEST_SIDE / 4, TEST_SIDE * 3 / 4] {
        assert_eq!(canvas.get_pixel(x, TEST_SIDE / 2).0, [255, 0, 0], "at x={x}");
    }

    assert!(compose_cover(&[broken], TEST_SIDE).is_none(), "nothing readable is still nothing");
    Ok(())
}

/// The forged-header guard every other decode in the tree carries. One pixel over on
/// the long axis only, `decode_capped` bounding each dimension independently — a
/// square at the cap would be a 200 MB fixture.
#[test]
fn a_source_past_the_decode_cap_is_refused() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let ok = solid_source(tmp.path(), "ok.png", [255, 0, 0], 64, 64)?;
    let over = solid_source(tmp.path(), "over.png", [0, 255, 0], MAX_SOURCE_DIM + 1, 1)?;

    assert!(compose_cover(std::slice::from_ref(&over), TEST_SIDE).is_none());

    // Beside a readable source the refusal is just a source that won't decode, so it drops out
    // like any other — the guard still holds, the green never reaching the canvas.
    let canvas = compose_cover(&[ok, over], TEST_SIDE)
        .ok_or_else(|| AppError::Validation("the readable source must still compose".into()))?;
    for x in [TEST_SIDE / 4, TEST_SIDE * 3 / 4] {
        assert_eq!(canvas.get_pixel(x, TEST_SIDE / 2).0, [255, 0, 0], "at x={x}");
    }
    Ok(())
}

/// `compose_artwork` is the strict half, and the asymmetry is deliberate: it bakes a file the
/// mosaic picker has already previewed slot for slot, so a source dropping out would persist a
/// collage that isn't the one the user chose.
#[test]
fn the_persisted_collage_refuses_what_the_hero_would_drop() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let good = solid_source(tmp.path(), "good.png", [255, 0, 0], 64, 64)?;

    let broken = tmp.path().join("broken.png");
    std::fs::write(&broken, b"not an image")?;

    assert!(compose_artwork(&[good.clone(), broken], tmp.path()).is_none());
    // The strictness is about the *missing* source, not about composing at all.
    assert!(compose_artwork(&[good], tmp.path()).is_some());
    Ok(())
}
