use super::*;
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::setup_seeded_db;
use crate::error::AppError;

#[test]
fn build_fts_query_single_word() {
    assert_eq!(build_fts_query("hello"), r#""hello"*"#);
}

#[test]
fn build_fts_query_multiple_words() {
    assert_eq!(build_fts_query("hello world"), r#""hello"* "world"*"#);
}

#[test]
fn build_fts_query_with_quotes() {
    assert_eq!(build_fts_query(r#"word"s"#), r#""word""s"*"#);
}

#[test]
fn build_fts_query_empty_input() {
    assert_eq!(build_fts_query(""), "");
    assert_eq!(build_fts_query("   "), "");
}

// === Async DB tests for search_all ===

#[tokio::test]
async fn search_all_empty_query_returns_empty() -> Result<(), AppError> {
    let db = DbPool::test_pool().await;
    let results = queries::search::search_all(&db, "   ").await?;
    assert!(results.tracks.is_empty());
    assert!(results.albums.is_empty());
    assert!(results.artists.is_empty());
    Ok(())
}

#[tokio::test]
async fn search_all_finds_track_by_title() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Alpha").await?;
    assert!(!results.tracks.is_empty());
    assert!(results.tracks.iter().any(|t| t.title == "Alpha Song"));
    Ok(())
}

#[tokio::test]
async fn search_all_finds_albums_and_artists() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "Album One").await?;
    assert!(!results.albums.is_empty());
    assert!(results.albums.iter().any(|a| a.name == "Album One"));
    Ok(())
}

#[tokio::test]
async fn search_all_no_results() -> Result<(), AppError> {
    let db = setup_seeded_db().await?;
    let results = queries::search::search_all(&db, "zzz_nonexistent_xyz").await?;
    assert!(results.tracks.is_empty());
    assert!(results.albums.is_empty());
    assert!(results.artists.is_empty());
    Ok(())
}
