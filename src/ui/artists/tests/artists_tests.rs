use super::*;

/// Minimal `ArtistStats` builder — only the fields the grid filter / sort
/// read matter.
fn artist(
    id: i64,
    name: &str,
    sort_name: Option<&str>,
    track_count: i32,
    album_count: i32,
) -> ArtistStats {
    ArtistStats {
        id,
        name: name.to_string(),
        sort_name: sort_name.map(str::to_string),
        musicbrainz_id: None,
        image_path: None,
        track_count,
        album_count,
        total_duration_ms: 0,
    }
}

fn names(data: &GridData, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|&i| data.artists[i].name.clone())
        .collect()
}

#[test]
fn grid_data_precomputes_lowercased_keys() {
    let data = GridData::new(vec![artist(1, "The BEATLES", Some("Beatles, The"), 0, 0)]);
    assert_eq!(data.keys.len(), 1);
    assert_eq!(data.keys[0].name_lc, "the beatles");
    assert_eq!(data.keys[0].sort_name_lc, "beatles, the");
}

#[test]
fn grid_data_sort_name_falls_back_to_name_when_missing() {
    let data = GridData::new(vec![artist(1, "Radiohead", None, 0, 0)]);
    assert_eq!(data.keys[0].name_lc, "radiohead");
    assert_eq!(data.keys[0].sort_name_lc, "radiohead");
}

#[test]
fn compute_indices_with_empty_filter_keeps_all_artists() {
    let data = GridData::new(vec![
        artist(1, "Alpha", None, 0, 0),
        artist(2, "Bravo", None, 0, 0),
    ]);
    let idx = compute_indices(&data, "name", "asc", "");
    assert_eq!(names(&data, &idx), ["Alpha", "Bravo"]);
}

#[test]
fn compute_indices_filter_matches_name_or_sort_name_case_insensitively() {
    let data = GridData::new(vec![
        artist(1, "The Beatles", Some("Beatles, The"), 0, 0),
        artist(2, "Radiohead", None, 0, 0),
        artist(3, "The Rolling Stones", Some("Rolling Stones, The"), 0, 0),
    ]);
    // Name substring, case-insensitive.
    let by_name = compute_indices(&data, "name", "asc", "BEATLES");
    assert_eq!(names(&data, &by_name), ["The Beatles"]);
    // sort_name substring (matches "rolling" inside "Rolling Stones, The"
    // even though `name` is "The Rolling Stones").
    let by_sort = compute_indices(&data, "name", "asc", "rolling");
    assert_eq!(names(&data, &by_sort), ["The Rolling Stones"]);
}

#[test]
fn compute_indices_track_count_sort_breaks_ties_by_name_and_honours_dir() {
    let data = GridData::new(vec![
        artist(1, "Later", None, 50, 0),
        artist(2, "Earlier", None, 10, 0),
        artist(3, "AlsoEarlier", None, 10, 0),
    ]);
    let asc = compute_indices(&data, "track_count", "asc", "");
    assert_eq!(names(&data, &asc), ["AlsoEarlier", "Earlier", "Later"]);
    let desc = compute_indices(&data, "track_count", "desc", "");
    assert_eq!(names(&data, &desc), ["Later", "Earlier", "AlsoEarlier"]);
}

#[test]
fn compute_indices_album_count_sort_breaks_ties_by_name() {
    let data = GridData::new(vec![
        artist(1, "Zed", None, 0, 3),
        artist(2, "Beta", None, 0, 1),
        artist(3, "Alpha", None, 0, 1),
    ]);
    let asc = compute_indices(&data, "album_count", "asc", "");
    assert_eq!(names(&data, &asc), ["Alpha", "Beta", "Zed"]);
}

#[test]
fn grid_index_cache_matches_only_identical_filter_and_sort() {
    let c = GridIndexCache {
        filter: "x".into(),
        sort_field: "name".into(),
        sort_dir: "asc".into(),
        indices: vec![],
    };
    assert!(c.matches("x", "name", "asc"));
    assert!(!c.matches("y", "name", "asc"));
    assert!(!c.matches("x", "track_count", "asc"));
    assert!(!c.matches("x", "name", "desc"));
}

#[test]
fn compute_artist_cover_cap_clamps_and_scales_with_resolution() {
    // A tiny display can't fill many cards — clamps to the floor (32).
    assert_eq!(compute_artist_cover_cap(640, 480).get(), 32);
    // A 4K panel shows far more than the ceiling — clamps to the cap (96).
    assert_eq!(compute_artist_cover_cap(3840, 2160).get(), 96);
    // A mid-range display lands strictly between the clamps...
    let mid = compute_artist_cover_cap(1920, 1080).get();
    assert!(
        mid > 32 && mid < 96,
        "1080p cap {mid} should sit between the clamps"
    );
    // ...and the cap is monotonic in display area.
    let small = compute_artist_cover_cap(1280, 720).get();
    let large = compute_artist_cover_cap(2560, 1440).get();
    assert!(small <= mid && mid <= large);
}
