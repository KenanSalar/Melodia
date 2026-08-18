//! The reference set the artwork sweep deletes against.
//!
//! A missing column here is not a compile error and not a wrong-looking result — it is a sweep
//! that unlinks live artwork, so both halves are pinned: that the SQL names all four columns, and
//! that the one reachable through no other column comes back.

use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::{insert_test_track, setup_seeded_db};
use crate::error::AppError;

use super::{REFERENCED_PATHS, REPOINT_UPDATES};

/// Every column in the schema that stores a path into the artwork directories.
///
/// A fifth would have to be added here *and* to the query. Spelled as `(table, column)` so the
/// walk can't be satisfied by a column name that happens to appear in another arm's text —
/// `artwork_path` is on two tables.
const ARTWORK_COLUMNS: [(&str, &str); 4] = [
    ("tracks", "artwork_path"),
    ("albums", "artwork_path"),
    ("artists", "image_path"),
    ("playlists", "thumbnail_path"),
];

/// The sweep keeps only what this query returns, so a column it forgets is artwork deleted while
/// a row still points at it. `playlists.thumbnail_path` is the one that was missing from the
/// first draft, and the one a reviewer is least likely to miss twice.
#[test]
fn the_reference_query_names_every_artwork_column() {
    for (table, column) in ARTWORK_COLUMNS {
        assert!(
            REFERENCED_PATHS.contains(&format!("{column} FROM {table}")),
            "the reference query no longer selects {table}.{column}, so a sweep will delete \
             artwork that column still points at"
        );
    }
    assert_eq!(
        REFERENCED_PATHS.matches("SELECT").count(),
        ARTWORK_COLUMNS.len(),
        "the query has an arm `ARTWORK_COLUMNS` doesn't name, or names one twice"
    );
}

/// The write side has the same shape of failure as the read side: a column the renormalize pass
/// forgets to re-point keeps naming a file that pass has just orphaned, and the next sweep
/// deletes it out from under the row.
#[test]
fn the_repoint_updates_cover_every_artwork_column() {
    for (table, column) in ARTWORK_COLUMNS {
        assert!(
            REPOINT_UPDATES
                .iter()
                .any(|sql| sql.contains(&format!("UPDATE {table} SET {column} ="))),
            "nothing re-points {table}.{column}, so a renormalized cover leaves it dangling"
        );
    }
    assert_eq!(
        REPOINT_UPDATES.len(),
        ARTWORK_COLUMNS.len(),
        "the update list and the column ledger have drifted apart"
    );
}

/// A custom playlist mosaic is written by `compose_artwork` and pointed at by nothing else, so it
/// is the case a three-column union silently blanks.
#[tokio::test]
async fn a_composite_referenced_only_by_a_playlist_is_still_referenced() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    let composite = "/data/artwork/33fb807d1f1b7cbb.jpg";

    let playlist = queries::playlist::create_playlist(&db, "Mosaic", None).await?;
    queries::playlist::set_playlist_custom_thumbnail(&db, playlist.id, composite).await?;

    let referenced = queries::artwork::referenced_filenames(&db).await?;

    assert!(
        referenced.contains("33fb807d1f1b7cbb.jpg"),
        "a composite reachable only through `playlists.thumbnail_path` must survive the sweep; \
         got {referenced:?}"
    );
    Ok(())
}

/// The sweep compares against a directory listing, so a stored absolute path has to reduce to the
/// name on disk — including for a row written before the data directory moved.
#[tokio::test]
async fn paths_are_reduced_to_the_name_on_disk() -> Result<(), AppError> {
    // Seeded rather than bare: `tracks.folder_id` is a foreign key, and the seeded tracks carry
    // no artwork so they contribute nothing to the set under test.
    let db = setup_seeded_db().await?;
    let track_id = insert_test_track(&db, "/music/a.mp3", "A", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    let stored = "/some/other/root/artwork/abcdef0123456789.png";
    queries::track::set_track_artwork(&mut tx, &[track_id], Some(stored)).await?;
    tx.commit().await?;

    let referenced = queries::artwork::referenced_filenames(&db).await?;

    assert!(
        referenced.contains("abcdef0123456789.png"),
        "expected the basename, got {referenced:?}"
    );
    Ok(())
}

/// An empty string is as good as NULL on these columns and must not reach the set — it reduces to
/// no basename at all, and a `""` entry would match nothing while looking like it had.
#[tokio::test]
async fn blank_and_missing_paths_contribute_nothing() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let track_id = insert_test_track(&db, "/music/b.mp3", "B", "Artist", "Album", "Rock").await?;

    let mut tx = db.write().begin().await?;
    queries::track::set_track_artwork(&mut tx, &[track_id], Some("")).await?;
    tx.commit().await?;

    assert!(queries::artwork::referenced_filenames(&db).await?.is_empty());
    Ok(())
}
