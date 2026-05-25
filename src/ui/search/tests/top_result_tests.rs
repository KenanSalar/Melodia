//! Unit tests for the 6-step Top Result ranking. The fixtures keep the
//! album/artist sets small and deliberately ambiguous so each ranking
//! tier is exercised independently — a single regression in the
//! function body would flip the corresponding tier's expected winner.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{TopKind, compute_top_result};
use crate::database::queries::SearchResults;
use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;

fn album(id: i64, name: &str) -> AlbumStats {
    AlbumStats {
        id,
        name: name.to_owned(),
        sort_name: None,
        artist_id: 1,
        artist_name: "X".to_owned(),
        year: None,
        disc_count: None,
        is_compilation: false,
        musicbrainz_id: None,
        artwork_path: None,
        track_count: 1,
        total_duration_ms: 0,
    }
}

fn artist(id: i64, name: &str) -> ArtistStats {
    ArtistStats {
        id,
        name: name.to_owned(),
        sort_name: None,
        musicbrainz_id: None,
        image_path: None,
        track_count: 1,
        album_count: 1,
        total_duration_ms: 0,
    }
}

fn results(albums: Vec<AlbumStats>, artists: Vec<ArtistStats>) -> SearchResults {
    SearchResults {
        tracks: Vec::new(),
        albums,
        artists,
    }
}

#[test]
fn empty_query_returns_none() {
    let r = results(vec![album(1, "Metal Album")], vec![artist(1, "Metal")]);
    assert!(compute_top_result(&r, "").is_none());
    assert!(compute_top_result(&r, "   ").is_none());
}

#[test]
fn empty_results_returns_none() {
    let r = results(Vec::new(), Vec::new());
    assert!(compute_top_result(&r, "metal").is_none());
}

#[test]
fn tier_1_exact_album_wins_over_exact_artist() {
    // Both an exact-album and an exact-artist match are present.
    // Tier 1 (album) should beat tier 2 (artist).
    let r = results(
        vec![album(10, "Metal")],
        vec![artist(20, "Metal"), artist(21, "Metallica")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
    assert_eq!(top.id, 10);
}

#[test]
fn tier_2_exact_artist_wins_when_no_exact_album() {
    let r = results(
        vec![album(10, "Metal Album"), album(11, "Metallic Sounds")],
        vec![artist(20, "Metal")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Artist);
    assert_eq!(top.id, 20);
}

#[test]
fn tier_3_album_starts_with_wins_when_no_exact() {
    let r = results(
        vec![album(10, "Metal Album")],
        vec![artist(20, "Metallica")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
    assert_eq!(top.id, 10);
}

#[test]
fn tier_4_artist_starts_with_when_no_starts_with_album() {
    let r = results(
        // Album exists but doesn't start with the query.
        vec![album(10, "Heavy Metal")],
        vec![artist(20, "Metallica")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Artist);
    assert_eq!(top.id, 20);
}

#[test]
fn tier_5_first_album_when_no_exact_or_starts_with() {
    let r = results(
        vec![album(10, "Heavy Metal"), album(11, "Death Metal")],
        vec![artist(20, "Iron Maiden")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
    assert_eq!(top.id, 10); // first in vec
}

#[test]
fn tier_6_first_artist_when_no_albums() {
    let r = results(
        Vec::new(),
        vec![artist(20, "Iron Maiden"), artist(21, "Megadeth")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Artist);
    assert_eq!(top.id, 20);
}

#[test]
fn case_insensitive_exact_match() {
    let r = results(vec![album(10, "METAL")], vec![]);
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
}

#[test]
fn whitespace_trimmed_from_query() {
    let r = results(vec![album(10, "Metal")], vec![]);
    let top = compute_top_result(&r, "  metal  ").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
}

#[test]
fn subtitle_for_artist_top_uses_album_count() {
    let mut a = artist(20, "Metallica");
    a.album_count = 11;
    let r = results(vec![], vec![a]);
    let top = compute_top_result(&r, "metallica").expect("top result");
    assert_eq!(top.subtitle, "11 albums");
}

#[test]
fn subtitle_for_album_top_uses_artist_name() {
    let mut a = album(10, "Master of Puppets");
    a.artist_name = "Metallica".to_owned();
    let r = results(vec![a], vec![]);
    let top = compute_top_result(&r, "master of puppets").expect("top result");
    assert_eq!(top.subtitle, "Metallica");
}
