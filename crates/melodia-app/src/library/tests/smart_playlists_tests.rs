//! What a smart playlist's row takes from its rules on every read, its count and its automatic
//! cover, and what it leaves to the user.

use std::path::Path;

use super::*;
use crate::library::playlists;
use melodia_store::database::queries::fixtures::{insert_test_track, set_test_artwork};

const LIBRARY_ROOT: &str = "/music";

/// A smart playlist of the library's Rock tracks, `Alpha` and `Gamma`.
const ROCK_ONLY: &str =
    r#"{"rules":[{"field":"genre","op":"contains","value":{"kind":"text","value":"Rock"}}]}"#;

/// `Alpha` is Rock with no cover, `Beta` is Pop under one and `Gamma` is Rock under one, inserted
/// in that order. So the first cover the Rock rules reach is `Gamma`'s, behind a match without one
/// and past a cover outside the rules.
struct Library {
    db: DbPool,
    alpha: i64,
}

async fn library() -> Result<Library, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, LIBRARY_ROOT, true).await?;
    let alpha = stage(&db, "Alpha", "Rock").await?;
    let beta = stage(&db, "Beta", "Pop").await?;
    let gamma = stage(&db, "Gamma", "Rock").await?;
    set_test_artwork(&db, beta, "/artwork/beta.jpg").await?;
    set_test_artwork(&db, gamma, "/artwork/gamma.jpg").await?;
    Ok(Library { db, alpha })
}

async fn stage(db: &DbPool, title: &str, genre: &str) -> Result<i64, AppError> {
    let path = Path::new(LIBRARY_ROOT).join(format!("{title}.mp3"));
    insert_test_track(db, &path.to_string_lossy(), title, "Artist", "Album", genre).await
}

/// A new Rock playlist's row, as the store reads it.
async fn rock_row(db: &DbPool) -> Result<PlaylistStats, AppError> {
    let rock = queries::playlist::create_smart_playlist(db, "Rock", None, ROCK_ONLY).await?;
    playlists::get_playlist_detail(db, rock.id).await
}

/// What a manual playlist's automatic thumbnail is too: the first track it reaches that has a
/// cover.
#[tokio::test]
async fn a_smart_playlist_takes_the_first_cover_its_rules_reach() -> Result<(), AppError> {
    let lib = library().await?;
    let mut rock = rock_row(&lib.db).await?;

    fill_automatic_thumbnail(&lib.db, &mut rock).await;

    assert_eq!(rock.thumbnail_path.as_deref(), Some("/artwork/gamma.jpg"));
    Ok(())
}

/// A picked mosaic and a Clear are both the user's choice, so the rules' cover replaces neither:
/// the Clear pins the placeholder.
#[tokio::test]
async fn a_smart_playlists_own_artwork_choice_is_left_alone() -> Result<(), AppError> {
    let lib = library().await?;
    let picked =
        queries::playlist::create_smart_playlist(&lib.db, "Picked", None, ROCK_ONLY).await?;
    queries::playlist::set_playlist_custom_thumbnail(&lib.db, picked.id, "/artwork/mosaic.jpg")
        .await?;
    let cleared =
        queries::playlist::create_smart_playlist(&lib.db, "Cleared", None, ROCK_ONLY).await?;
    playlists::update_playlist(&lib.db, cleared.id, "Cleared".to_owned(), None, Some(true)).await?;
    let cases = [
        (picked.id, Some("/artwork/mosaic.jpg"), "a picked mosaic"),
        (cleared.id, None, "a Clear"),
    ];
    for (id, kept, case) in cases {
        let mut row = playlists::get_playlist_detail(&lib.db, id).await?;

        fill_automatic_thumbnail(&lib.db, &mut row).await;

        assert_eq!(row.thumbnail_path.as_deref(), kept, "{case}");
    }
    Ok(())
}

/// The grid's and the export picker's read: the junction triggers leave a smart row's stats at 0,
/// and its stored thumbnail is never written.
#[tokio::test]
async fn a_smart_row_reads_its_count_and_cover_from_its_rules() -> Result<(), AppError> {
    let lib = library().await?;
    let mut rows = [rock_row(&lib.db).await?];

    fill_from_rules(&lib.db, &mut rows).await;

    let [rock] = rows;
    assert_eq!((rock.track_count, rock.thumbnail_path.as_deref()), (2, Some("/artwork/gamma.jpg")));
    Ok(())
}

/// Recounted, a manual row would be counted over no rules, which match the whole library.
#[tokio::test]
async fn a_manual_row_is_left_as_read() -> Result<(), AppError> {
    let lib = library().await?;
    let mix =
        queries::playlist::create_playlist_with_tracks(&lib.db, "Mix", None, &[lib.alpha]).await?;
    let as_read = playlists::get_playlist_detail(&lib.db, mix).await?;
    let mut rows = [as_read.clone()];

    fill_from_rules(&lib.db, &mut rows).await;

    assert_eq!(rows, [as_read]);
    Ok(())
}
