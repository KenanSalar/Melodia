use crate::error::AppError;

use super::*;

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

// ── init_*_cache ──

#[test]
fn init_artwork_cache_creates_dir() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let result = init_artwork_cache(tmp.path());
    assert!(result.is_ok());
    assert!(tmp.path().join("artwork").is_dir());
    Ok(())
}

#[test]
fn init_artists_cache_creates_dir() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let result = init_artists_cache(tmp.path());
    assert!(result.is_ok());
    assert!(tmp.path().join("artists").is_dir());
    Ok(())
}

#[test]
fn init_artwork_cache_idempotent() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let r1 = init_artwork_cache(tmp.path());
    let r2 = init_artwork_cache(tmp.path());
    assert!(r1.is_ok());
    assert!(r2.is_ok());
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

    let source = tmp.path().join("cover.jpg");
    std::fs::write(&source, b"fake image data")?;

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

    let source1 = tmp.path().join("a.png");
    let source2 = tmp.path().join("b.png");
    std::fs::write(&source1, b"identical content")?;
    std::fs::write(&source2, b"identical content")?;

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
    std::fs::write(album_dir.join("cover.jpg"), b"album art")?;

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
    std::fs::write(album_dir.join("cover.jpg"), b"album art")?;

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
    let result = extract_and_cache_artwork(&tag, &artwork_dir);
    assert!(result.is_none());
    Ok(())
}

// ── compose_cover ──

/// A solid-colour square PNG, so a sampled pixel names the source it came from.
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

/// The four layouts, pinned against composed pixels. This is the arrangement `CoverMosaic`
/// used to draw in Slint, and the collage is now the only place it is stated.
///
/// The sample points are spelled here rather than read off `COMPOSITE_LAYOUTS`: sampling the
/// table the compose loop walks proves only that the loop honours it, and passes just as
/// happily when two of its rows are swapped.
#[test]
fn each_layout_puts_every_source_in_its_own_rect() -> Result<(), AppError> {
    const COLOURS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]];

    const NEAR: u32 = COMPOSITE_SIZE / 4;
    const FAR: u32 = COMPOSITE_SIZE * 3 / 4;
    const MID: u32 = COMPOSITE_SIZE / 2;
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
        let canvas = compose_cover(&sources[..count])
            .ok_or_else(|| AppError::Validation(format!("compose of {count} returned None")))?;
        assert_eq!((canvas.width(), canvas.height()), (COMPOSITE_SIZE, COMPOSITE_SIZE));

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

#[test]
fn compose_cover_refuses_a_set_it_has_no_layout_for() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let one = solid_source(tmp.path(), "one.png", [255, 0, 0], 64, 64)?;

    assert!(compose_cover(&[]).is_none());
    assert!(compose_cover(&vec![one; 5]).is_none());
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

    let canvas = compose_cover(&[good, broken.clone()])
        .ok_or_else(|| AppError::Validation("a readable source must still compose".into()))?;
    // The 1-up layout rather than the 2-up: the survivor takes the half the broken source would
    // have had, which is what separates this from painting a blank quarter.
    for x in [COMPOSITE_SIZE / 4, COMPOSITE_SIZE * 3 / 4] {
        assert_eq!(canvas.get_pixel(x, COMPOSITE_SIZE / 2).0, [255, 0, 0], "at x={x}");
    }

    assert!(compose_cover(&[broken]).is_none(), "nothing readable is still nothing");
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

    assert!(compose_cover(std::slice::from_ref(&over)).is_none());

    // Beside a readable source the refusal is just a source that won't decode, so it drops out
    // like any other — the guard still holds, the green never reaching the canvas.
    let canvas = compose_cover(&[ok, over])
        .ok_or_else(|| AppError::Validation("the readable source must still compose".into()))?;
    for x in [COMPOSITE_SIZE / 4, COMPOSITE_SIZE * 3 / 4] {
        assert_eq!(canvas.get_pixel(x, COMPOSITE_SIZE / 2).0, [255, 0, 0], "at x={x}");
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
