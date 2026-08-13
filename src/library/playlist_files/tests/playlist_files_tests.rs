use std::collections::HashSet;

#[allow(clippy::wildcard_imports)]
use super::*;
use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::tests::helpers::insert_test_track;
use crate::error::AppError;

fn entry(path: &str, hash: Option<&str>) -> m3u::ParsedEntry {
    m3u::ParsedEntry {
        path: path.to_owned(),
        hash: hash.map(ToOwned::to_owned),
        duration_secs: None,
        display: None,
    }
}

/// Seed a pool with three tracks; returns `(db, id_a, id_b, id_c)`.
/// `make_test_metadata` sets `file_hash = blake3(title)`, so each track's
/// hash is deterministic from its title.
async fn seed() -> Result<(DbPool, i64, i64, i64), AppError> {
    let db = DbPool::test_pool().await;
    queries::folder::insert_folder(&db, "/music", true).await?;
    let a =
        insert_test_track(&db, "/music/a.mp3", "Alpha Song", "Artist A", "Album", "Rock").await?;
    let b = insert_test_track(&db, "/music/b.mp3", "Beta Song", "Artist B", "Album", "Pop").await?;
    let c =
        insert_test_track(&db, "/music/c.mp3", "Gamma Song", "Artist A", "Album", "Rock").await?;
    Ok((db, a, b, c))
}

fn hash_of(title: &str) -> String {
    blake3::hash(title.as_bytes()).to_hex().to_string()
}

#[tokio::test]
async fn match_entries_by_path() -> Result<(), AppError> {
    let (db, a, b, c) = seed().await?;
    let entries = [
        entry("/music/a.mp3", None),
        entry("/music/b.mp3", None),
        entry("/music/c.mp3", None),
    ];
    let out = match_entries(&db, &entries, None).await?;
    assert_eq!(out.matched_by_path, 3);
    assert_eq!(out.matched_by_hash, 0);
    assert_eq!(out.missing, 0);
    assert_eq!(out.ordered_ids, vec![a, b, c]);
    Ok(())
}

#[tokio::test]
async fn match_entries_by_hash_when_path_is_stale() -> Result<(), AppError> {
    let (db, a, _b, _c) = seed().await?;
    // Path no longer exists, but the hash matches track A (title "Alpha Song").
    let entries = [entry("/moved/elsewhere.mp3", Some(&hash_of("Alpha Song")))];
    let out = match_entries(&db, &entries, None).await?;
    assert_eq!(out.matched_by_path, 0);
    assert_eq!(out.matched_by_hash, 1);
    assert_eq!(out.missing, 0);
    assert_eq!(out.ordered_ids, vec![a]);
    Ok(())
}

#[tokio::test]
async fn match_entries_reports_missing() -> Result<(), AppError> {
    let (db, ..) = seed().await?;
    let entries = [entry("/nope/gone.mp3", Some(&hash_of("Not In Library")))];
    let out = match_entries(&db, &entries, None).await?;
    assert_eq!(out.matched_by_path, 0);
    assert_eq!(out.matched_by_hash, 0);
    assert_eq!(out.missing, 1);
    assert!(out.ordered_ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn match_entries_preserves_file_order_across_passes() -> Result<(), AppError> {
    let (db, a, b, c) = seed().await?;
    // Order: hash-match(b), path-match(a), missing, path-match(c).
    let entries = [
        entry("/stale/b.mp3", Some(&hash_of("Beta Song"))),
        entry("/music/a.mp3", None),
        entry("/gone.mp3", None),
        entry("/music/c.mp3", None),
    ];
    let out = match_entries(&db, &entries, None).await?;
    assert_eq!(out.matched_by_path, 2);
    assert_eq!(out.matched_by_hash, 1);
    assert_eq!(out.missing, 1);
    // Misses omitted, surviving ids keep file order.
    assert_eq!(out.ordered_ids, vec![b, a, c]);
    Ok(())
}

#[tokio::test]
async fn match_entries_does_not_dedup_repeated_tracks() -> Result<(), AppError> {
    // match_entries preserves every match; dedup is add_tracks_to_playlist's job.
    let (db, a, ..) = seed().await?;
    let entries = [entry("/music/a.mp3", None), entry("/music/a.mp3", None)];
    let out = match_entries(&db, &entries, None).await?;
    assert_eq!(out.ordered_ids, vec![a, a]);
    Ok(())
}

#[tokio::test]
async fn match_entries_resolves_relative_paths_against_base() -> Result<(), AppError> {
    let (db, a, ..) = seed().await?;
    let base = std::path::Path::new("/music");
    let entries = [entry("a.mp3", None)];
    let out = match_entries(&db, &entries, Some(base)).await?;
    assert_eq!(out.matched_by_path, 1);
    assert_eq!(out.ordered_ids, vec![a]);
    Ok(())
}

// --- Filename sanitization ---

#[test]
fn sanitize_stem_replaces_illegal_chars() {
    assert_eq!(sanitize_stem("a/b:c?"), "a_b_c_");
    assert_eq!(sanitize_stem("a\nb"), "a_b");
    assert_eq!(sanitize_stem("x<y>z|w*"), "x_y_z_w_");
    // Illegal chars become underscores rather than triggering the fallback.
    assert_eq!(sanitize_stem("///"), "___");
}

#[test]
fn sanitize_stem_falls_back_when_empty() {
    assert_eq!(sanitize_stem(""), "playlist");
    assert_eq!(sanitize_stem("   "), "playlist");
}

#[test]
fn sanitize_stem_guards_windows_reserved_names() {
    assert_eq!(sanitize_stem("CON"), "_CON");
    assert_eq!(sanitize_stem("con"), "_con");
    assert_eq!(sanitize_stem("Lpt9"), "_Lpt9");
    // Not reserved — left alone.
    assert_eq!(sanitize_stem("CONsole"), "CONsole");
}

#[test]
fn sanitize_stem_strips_trailing_dots_and_spaces() {
    assert_eq!(sanitize_stem("name...  "), "name");
    assert_eq!(sanitize_stem("  spaced  "), "spaced");
}

#[test]
fn sanitize_stem_caps_length() {
    let long = "x".repeat(300);
    assert_eq!(sanitize_stem(&long).chars().count(), MAX_STEM_CHARS);
}

#[test]
fn unique_filename_disambiguates_collisions() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    let mut used: HashSet<String> = HashSet::new();
    assert_eq!(unique_filename(dir.path(), "Road", &mut used), "Road.m3u8");
    assert_eq!(unique_filename(dir.path(), "Road", &mut used), "Road (2).m3u8");
    assert_eq!(unique_filename(dir.path(), "Road", &mut used), "Road (3).m3u8");
    Ok(())
}

#[test]
fn unique_filename_avoids_existing_files() -> Result<(), AppError> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("Mix.m3u8"), b"x")?;
    let mut used: HashSet<String> = HashSet::new();
    assert_eq!(unique_filename(dir.path(), "Mix", &mut used), "Mix (2).m3u8");
    Ok(())
}
