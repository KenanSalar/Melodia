//! What a scan's repair does to a library reference whose stored file is not where it says.

use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tokio::sync::watch;

use super::forget_missing;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_store::database::queries::fixtures::{insert_test_track, set_test_artwork};
use melodia_store::database::{DbPool, queries};

const STORED_NAME: &str = "33fb807d1f1b7cbb.jpg";

/// A data root with its stores created, one folder and one track in it.
struct Library {
    db: DbPool,
    paths: Paths,
    track: i64,
    restoring: watch::Sender<u32>,
    tmp: TempDir,
}

impl Library {
    async fn new() -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let paths = Paths::rooted_at(tmp.path().to_path_buf());
        paths.create_dirs()?;
        let db = DbPool::test_pool().await?;
        let music = tmp.path().join("music");
        queries::folder::insert_folder(&db, &as_str(&music), true).await?;
        let track =
            insert_test_track(&db, &as_str(&music.join("a.mp3")), "A", "Artist", "Album", "Rock")
                .await?;
        Ok(Self { db, paths, track, restoring: watch::Sender::new(0), tmp })
    }

    /// Where the store of a data root that has since moved kept `name`.
    fn moved_root(&self, store: &str, name: &str) -> PathBuf {
        self.tmp.path().join("old-root").join(store).join(name)
    }

    async fn set_cover(&self, track: i64, path: &Path) -> Result<(), AppError> {
        set_test_artwork(&self.db, track, &as_str(path)).await
    }

    async fn cover(&self) -> Result<Option<String>, AppError> {
        Ok(sqlx::query_scalar("SELECT artwork_path FROM tracks WHERE id = ?")
            .bind(self.track)
            .fetch_one(self.db.read())
            .await?)
    }
}

fn as_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[tokio::test]
async fn a_reference_to_a_file_still_in_the_store_is_left_alone() -> Result<(), AppError> {
    let library = Library::new().await?;
    let stored = library.paths.artwork_dir.join(STORED_NAME);
    std::fs::write(&stored, b"a cover")?;
    library.set_cover(library.track, &stored).await?;

    let notice = forget_missing(&library.db, &library.paths, &library.restoring).await?;

    assert_eq!((notice.is_some(), library.cover().await?), (false, Some(as_str(&stored))));
    Ok(())
}

/// The file is in the current store under the same content-addressed name, and clearing the row
/// would cost a re-parse, an artist-image refetch or a custom playlist image outright.
#[tokio::test]
async fn a_reference_under_a_moved_data_folder_is_repointed_rather_than_cleared()
-> Result<(), AppError> {
    let library = Library::new().await?;
    let current = library.paths.artwork_dir.join(STORED_NAME);
    std::fs::write(&current, b"a cover")?;
    library.set_cover(library.track, &library.moved_root("artwork", STORED_NAME)).await?;

    let notice = forget_missing(&library.db, &library.paths, &library.restoring).await?;

    assert_eq!((notice.is_some(), library.cover().await?), (false, Some(as_str(&current))));
    Ok(())
}

/// An artist's photo lives in a store of its own, and missing it there would clear every artist
/// image after a data folder move, each one a network fetch to get back.
#[tokio::test]
async fn an_artist_image_under_a_moved_data_folder_is_found_in_its_own_store()
-> Result<(), AppError> {
    let library = Library::new().await?;
    let current = library.paths.artists_dir.join(STORED_NAME);
    std::fs::write(&current, b"a photo")?;
    sqlx::query("UPDATE artists SET image_path = ? WHERE name = 'Artist'")
        .bind(as_str(&library.moved_root("artists", STORED_NAME)))
        .execute(library.db.write())
        .await?;

    forget_missing(&library.db, &library.paths, &library.restoring).await?;

    let image: Option<String> =
        sqlx::query_scalar("SELECT image_path FROM artists WHERE name = 'Artist'")
            .fetch_one(library.db.read())
            .await?;
    assert_eq!(image, Some(as_str(&current)));
    Ok(())
}

/// Every refill fills an empty column and leaves a set one alone, and the scan's size and mtime
/// gate never re-reads an unchanged track, so only clearing the row lets a scan put it back.
#[tokio::test]
async fn a_reference_to_a_file_gone_from_every_store_is_cleared() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.set_cover(library.track, &library.paths.artwork_dir.join(STORED_NAME)).await?;

    forget_missing(&library.db, &library.paths, &library.restoring).await?;

    assert_eq!(library.cover().await?, None);
    Ok(())
}

/// The notice says covers are being restored for as long as the scan putting them back runs,
/// and a scan that stops or fails part way must not leave it up.
#[tokio::test]
async fn the_restore_notice_is_up_exactly_as_long_as_it_is_held() -> Result<(), AppError> {
    let library = Library::new().await?;
    library.set_cover(library.track, &library.paths.artwork_dir.join(STORED_NAME)).await?;

    let notice = forget_missing(&library.db, &library.paths, &library.restoring).await?;
    let while_held = *library.restoring.borrow();
    drop(notice);

    assert_eq!((while_held, *library.restoring.borrow()), (1, 0));
    Ok(())
}

/// The roll-ups run in the clearing transaction, so a cover another of the album's tracks still
/// holds is back before the scan starts rather than after it.
#[tokio::test]
async fn clearing_a_cover_hands_the_album_one_a_sibling_still_holds() -> Result<(), AppError> {
    let library = Library::new().await?;
    let gone = library.paths.artwork_dir.join(STORED_NAME);
    let held = library.paths.artwork_dir.join("4cccaf4d4b4cea11.jpg");
    std::fs::write(&held, b"a cover")?;
    let sibling = insert_test_track(
        &library.db,
        &as_str(&library.tmp.path().join("music").join("b.mp3")),
        "B",
        "Artist",
        "Album",
        "Rock",
    )
    .await?;
    library.set_cover(library.track, &gone).await?;
    library.set_cover(sibling, &held).await?;
    sqlx::query("UPDATE albums SET artwork_path = ? WHERE name = 'Album'")
        .bind(as_str(&gone))
        .execute(library.db.write())
        .await?;

    forget_missing(&library.db, &library.paths, &library.restoring).await?;

    let album: Option<String> =
        sqlx::query_scalar("SELECT artwork_path FROM albums WHERE name = 'Album'")
            .fetch_one(library.db.read())
            .await?;
    assert_eq!(album, Some(as_str(&held)));
    Ok(())
}
