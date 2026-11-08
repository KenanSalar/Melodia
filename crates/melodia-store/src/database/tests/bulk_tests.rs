use crate::database::queries;
use crate::database::queries::fixtures::insert_test_track;
use crate::database::{DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_execute, chunked_in_query, sql};
use melodia_core::error::AppError;

/// The ids among `ids` that name a track, as `chunked_in_query` reads them.
async fn ids_found(db: &DbPool, ids: Vec<i64>) -> Result<Vec<i64>, AppError> {
    db.read(move |conn| {
        chunked_in_query(conn, &ids, |ph| format!("SELECT id FROM tracks WHERE id IN ({ph})"))
    })
    .await
}

#[test]
fn max_binds_per_statement_is_999() {
    assert_eq!(MAX_BINDS_PER_STATEMENT, 999);
}

#[tokio::test]
async fn chunked_in_query_empty_input() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let result = ids_found(&db, Vec::new()).await?;
    assert!(result.is_empty());
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_small_set() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    queries::folder::insert_folder(&db, "/test", true).await?;

    let track_id =
        insert_test_track(&db, "/test/song.mp3", "Test Song", "Test Artist", "Test Album", "Rock")
            .await?;

    let ids = vec![track_id, 99999]; // one real, one missing
    let result = ids_found(&db, ids).await?;

    assert_eq!(result, [track_id]);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_single_item() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;
    let track_id = insert_test_track(&db, "/test/a.mp3", "A", "Art", "Alb", "Pop").await?;

    let result = ids_found(&db, vec![track_id]).await?;

    assert_eq!(result, [track_id]);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_preserves_all_results() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;

    let mut track_ids = Vec::new();
    for i in 0..5 {
        let id = insert_test_track(
            &db,
            &format!("/test/track_{i}.mp3"),
            &format!("Track {i}"),
            "Artist",
            "Album",
            "Rock",
        )
        .await?;
        track_ids.push(id);
    }

    let result = ids_found(&db, track_ids.clone()).await?;

    assert_eq!(result.len(), 5);
    for id in &track_ids {
        assert!(result.contains(id), "missing track id {id}");
    }
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_returns_every_row_of_a_fifty_id_set() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;

    let count = 50;
    let mut track_ids = Vec::with_capacity(count);
    for i in 0..count {
        let id = insert_test_track(
            &db,
            &format!("/test/track_{i}.mp3"),
            &format!("Track {i}"),
            "Artist",
            "Album",
            "Rock",
        )
        .await?;
        track_ids.push(id);
    }

    let result = ids_found(&db, track_ids).await?;

    assert_eq!(result.len(), count);
    Ok(())
}

/// One past the bind budget, so the statement runs as two. The count answered is every chunk's:
/// a scan's purge decides from it whether anything changed.
#[tokio::test]
async fn chunked_in_execute_counts_every_row_across_its_chunks() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let names: Vec<String> = (0..=MAX_BINDS_PER_STATEMENT).map(|n| format!("item {n}")).collect();
    let inserted = u64::try_from(names.len()).ok();

    let deleted = db
        .transaction(move |tx| {
            sql::execute(tx, "CREATE TEMP TABLE items (name TEXT)", ())?;
            for name in &names {
                sql::execute(tx, "INSERT INTO items (name) VALUES (?)", (name,))?;
            }
            chunked_in_execute(tx, &names, |placeholders| {
                format!("DELETE FROM items WHERE name IN ({placeholders})")
            })
        })
        .await?;

    assert_eq!(Some(deleted), inserted);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_with_string_binds() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/test", true).await?;
    insert_test_track(&db, "/test/song.mp3", "Song", "Art", "Alb", "Pop").await?;

    let paths = vec!["/test/song.mp3".to_owned(), "/test/missing.mp3".to_owned()];
    let result: Vec<i64> = db
        .read(move |conn| {
            chunked_in_query(conn, &paths, |ph| {
                format!("SELECT id FROM tracks WHERE file_path IN ({ph})")
            })
        })
        .await?;

    assert_eq!(result.len(), 1);
    Ok(())
}

#[tokio::test]
async fn chunked_in_query_all_missing() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let result = ids_found(&db, vec![9999_i64, 8888, 7777]).await?;
    assert!(result.is_empty());
    Ok(())
}
