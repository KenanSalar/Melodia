//! Tests for the two things this module decides for itself.
//!
//! The other eleven functions here are single `queries::playlist::*` calls with no transformation
//! over them, and `queries/tests/playlist_tests.rs` already covers that layer; a suite over them
//! would re-ask questions it has answered.

use tempfile::TempDir;

use super::*;
use melodia_artwork::media::image::artwork;
use melodia_testkit::ASSETS_DIR;

/// A staging root plus the artwork directory the composite is baked into.
fn staging() -> Result<(TempDir, PathBuf), AppError> {
    let tmp = TempDir::new()?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;
    Ok((tmp, artwork_dir))
}

fn solid_png(dir: &Path, name: &str) -> Result<String, AppError> {
    let path = dir.join(name);
    image::RgbImage::from_pixel(64, 64, image::Rgb([200, 60, 120]))
        .save(&path)
        .map_err(|e| AppError::Validation(format!("write {name}: {e}")))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Stage the silence fixture under `name`. It carries no title tag, so the row's title falls back
/// to the stem, which is what the drop ordering below sorts on.
fn drop_file(dir: &Path, name: &str) -> Result<String, AppError> {
    let dest = dir.join(name);
    std::fs::copy(PathBuf::from(ASSETS_DIR).join("silence.mp3"), &dest)?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Both sides of the mosaic picker's bound. Four is the number of slots the picker offers, so a
/// pick outside one-to-four is a caller that has lost track of its own grid rather than something
/// to clamp quietly onto a collage the user never saw.
#[tokio::test]
async fn a_thumbnail_pick_outside_one_to_four_images_is_refused() -> Result<(), AppError> {
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;
    let playlist = queries::playlist::create_playlist(&db, "Mosaic", None).await?;
    let pick = solid_png(tmp.path(), "a.png")?;

    for count in [0_usize, 5] {
        let picks = vec![pick.clone(); count];
        let refused = compose_thumbnail(&db, &artwork_dir, playlist.id, &picks).await;
        assert!(
            matches!(refused, Err(AppError::Validation(_))),
            "{count} images must not reach the compositor"
        );
    }
    Ok(())
}

/// The step either side of that bound, which is where a clamp written as `>= 4` would show.
#[tokio::test]
async fn a_thumbnail_pick_of_one_or_four_images_lands_on_the_row() -> Result<(), AppError> {
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;
    let pick = solid_png(tmp.path(), "a.png")?;

    for count in [1_usize, 4] {
        let name = format!("Mosaic of {count}");
        let playlist = queries::playlist::create_playlist(&db, &name, None).await?;
        let picks = vec![pick.clone(); count];

        let updated = compose_thumbnail(&db, &artwork_dir, playlist.id, &picks).await?;

        let stored = updated.thumbnail_path.unwrap_or_default();
        assert!(!stored.is_empty(), "{count} images must persist a composite");
        assert!(Path::new(&stored).exists(), "the row must name a file that is there: {stored}");
        assert!(
            updated.custom_thumbnail,
            "a composed mosaic is the user's pick, not a derived one"
        );
    }
    Ok(())
}

/// A drop arrives in whatever order the file manager built it, and the ids come back partly out
/// of a `HashMap`, so the positions persisted here are only right if the batch is sorted first.
/// Sorted *naturally*: under a plain byte compare "10" sorts before "9" and the playlist reads
/// wrong for every album anyone has ever numbered.
#[tokio::test]
async fn a_dropped_batch_lands_in_natural_order_rather_than_the_order_it_arrived()
-> Result<(), AppError> {
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;
    let playlist = queries::playlist::create_playlist(&db, "Dropped", None).await?;

    let dropped = vec![
        drop_file(tmp.path(), "foo.mp3")?,
        drop_file(tmp.path(), "10.mp3")?,
        drop_file(tmp.path(), "9.mp3")?,
    ];

    let result =
        import_into_playlist(&db, &artwork_dir, &artwork::new_cover_cache(), playlist.id, &dropped)
            .await?;
    assert_eq!(result.added_count, 3);

    let titles: Vec<String> = queries::playlist::get_playlist_tracks_for_list(&db, playlist.id)
        .await?
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(titles, ["9", "10", "foo"], "natord orders the drop, not the OS and not the id map");
    Ok(())
}

/// A drop of nothing importable must leave the playlist alone rather than adding a row per
/// refused path, since the caller reports `added_count` to the user as what it gained.
#[tokio::test]
async fn a_drop_with_nothing_importable_adds_nothing_to_the_playlist() -> Result<(), AppError> {
    let (tmp, artwork_dir) = staging()?;
    let db = DbPool::test_pool().await?;
    let playlist = queries::playlist::create_playlist(&db, "Untouched", None).await?;

    let notes = tmp.path().join("notes.txt");
    std::fs::write(&notes, b"not audio")?;
    let dropped = vec![notes.to_string_lossy().into_owned()];

    let result =
        import_into_playlist(&db, &artwork_dir, &artwork::new_cover_cache(), playlist.id, &dropped)
            .await?;

    assert_eq!(result.added_count, 0);
    assert_eq!(result.imported_count, 0);
    assert_eq!(result.failed_paths.len(), 1);
    assert!(
        queries::playlist::get_playlist_tracks_for_list(&db, playlist.id).await?.is_empty(),
        "a refused drop must not leave the playlist holding anything"
    );
    Ok(())
}
