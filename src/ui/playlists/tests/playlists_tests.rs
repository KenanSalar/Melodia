//! The Playlists grid's filter + sort pass — the last of the four entity
//! grids to get one. Same three cases its siblings pin, since all four run
//! the identical `row_match::field_contains` walk over a single name.

use super::*;

/// Minimal `PlaylistStats` builder — only the fields the grid filter / sort
/// read matter. Regular playlist, so `smart_criteria` never parses.
fn playlist(id: i64, name: &str, track_count: i32, updated_at: &str) -> PlaylistStats {
    PlaylistStats {
        id,
        name: name.to_owned(),
        description: None,
        thumbnail_path: None,
        is_smart: false,
        smart_criteria: None,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: updated_at.to_owned(),
        custom_thumbnail: false,
        track_count,
        total_duration_ms: 0,
    }
}

fn names(data: &GridData, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|&i| data.playlists[i].name.clone()).collect()
}

#[test]
fn compute_indices_with_empty_filter_keeps_all_playlists() {
    let data = GridData::new(vec![
        playlist(1, "Alpha", 1, "2026-01-02T00:00:00Z"),
        playlist(2, "Bravo", 1, "2026-01-01T00:00:00Z"),
    ]);
    let idx = compute_indices(&data, "name", "asc", "");
    assert_eq!(names(&data, &idx), ["Alpha", "Bravo"]);
}

#[test]
fn compute_indices_filter_matches_name_case_insensitively() {
    let data = GridData::new(vec![
        playlist(1, "Late Night Drive", 1, "2026-01-03T00:00:00Z"),
        playlist(2, "Night Shift", 1, "2026-01-02T00:00:00Z"),
        playlist(3, "Morning Coffee", 1, "2026-01-01T00:00:00Z"),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "NIGHT");
    assert_eq!(names(&data, &by_name), ["Late Night Drive", "Night Shift"]);
}

#[test]
fn compute_indices_filter_ignores_accents_the_way_the_search_view_does() {
    // A playlist name is whatever the user typed, so it carries whatever
    // their keyboard makes easy — and the query later may not.
    let data = GridData::new(vec![
        playlist(1, "Café Sessions", 1, "2026-01-02T00:00:00Z"),
        playlist(2, "Morning Coffee", 1, "2026-01-01T00:00:00Z"),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "cafe");
    assert_eq!(names(&data, &by_name), ["Café Sessions"]);
}

#[test]
fn compute_indices_default_sort_is_most_recently_updated_first() {
    // `playlist_stats` already returns `updated_at DESC`, so the default arm
    // re-derives that order rather than leaving whatever the filter produced.
    let data = GridData::new(vec![
        playlist(1, "Oldest", 1, "2026-01-01T00:00:00Z"),
        playlist(2, "Newest", 1, "2026-03-01T00:00:00Z"),
        playlist(3, "Middle", 1, "2026-02-01T00:00:00Z"),
    ]);
    let desc = compute_indices(&data, "updated", "desc", "");
    assert_eq!(names(&data, &desc), ["Newest", "Middle", "Oldest"]);
    let asc = compute_indices(&data, "updated", "asc", "");
    assert_eq!(names(&data, &asc), ["Oldest", "Middle", "Newest"]);
}
