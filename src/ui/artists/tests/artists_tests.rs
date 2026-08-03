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
fn grid_data_precomputes_the_lowercased_sort_key() {
    let data = GridData::new(vec![artist(1, "The BEATLES", Some("Beatles, The"), 0, 0)]);
    assert_eq!(data.keys.len(), 1);
    assert_eq!(data.keys[0].name_lc, "the beatles");
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
        artist(3, "坂本龍一", Some("Sakamoto, Ryuichi"), 0, 0),
    ]);
    // Name substring, case-insensitive.
    let by_name = compute_indices(&data, "name", "asc", "BEATLES");
    assert_eq!(names(&data, &by_name), ["The Beatles"]);
    // The sort name is the only thing this needle can reach, which is the case
    // the arm exists for — an artist whose only Latin handle is its sort name.
    // A needle merely reordered out of the display name ("rolling" against
    // "Rolling Stones, The") matches `name` as well, so it pins nothing.
    let by_sort = compute_indices(&data, "name", "asc", "sakamoto");
    assert_eq!(names(&data, &by_sort), ["坂本龍一"]);
}

#[test]
fn compute_indices_filter_ignores_accents_the_way_the_search_view_does() {
    let data = GridData::new(vec![
        artist(1, "Björk", None, 0, 0),
        artist(2, "Sigur Rós", None, 0, 0),
    ]);
    assert_eq!(names(&data, &compute_indices(&data, "name", "asc", "bjork")), ["Björk"]);
    assert_eq!(names(&data, &compute_indices(&data, "name", "asc", "ros")), ["Sigur Rós"]);
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
