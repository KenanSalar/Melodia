use super::*;

/// Minimal `AlbumStats` builder — only the fields the grid filter / sort
/// read matter.
fn album(id: i64, name: &str, artist: &str, year: Option<i32>) -> AlbumStats {
    AlbumStats {
        id,
        name: name.to_string(),
        sort_name: None,
        artist_id: 1,
        artist_name: artist.to_string(),
        year,
        disc_count: None,
        is_compilation: false,
        musicbrainz_id: None,
        artwork_path: None,
        track_count: 1,
        total_duration_ms: 0,
    }
}

fn names(data: &GridData, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|&i| data.albums[i].name.clone()).collect()
}

#[test]
fn grid_data_precomputes_lowercased_keys() {
    let data = GridData::new(vec![album(1, "AbBey Road", "The BEATLES", None)]);
    assert_eq!(data.keys.len(), 1);
    assert_eq!(data.keys[0].name_lc, "abbey road");
    assert_eq!(data.keys[0].artist_lc, "the beatles");
}

#[test]
fn compute_indices_with_empty_filter_keeps_all_albums() {
    let data = GridData::new(vec![
        album(1, "Alpha", "X", None),
        album(2, "Bravo", "Y", None),
    ]);
    let idx = compute_indices(&data, "name", "asc", "");
    assert_eq!(names(&data, &idx), ["Alpha", "Bravo"]);
}

#[test]
fn compute_indices_filter_matches_name_or_artist_case_insensitively() {
    let data = GridData::new(vec![
        album(1, "Kind of Blue", "Miles Davis", None),
        album(2, "Blue Train", "John Coltrane", None),
        album(3, "Giant Steps", "John Coltrane", None),
    ]);
    // Album-name substring, case-insensitive.
    let by_name = compute_indices(&data, "name", "asc", "BLUE");
    assert_eq!(names(&data, &by_name), ["Blue Train", "Kind of Blue"]);
    // Artist-name substring.
    let by_artist = compute_indices(&data, "name", "asc", "coltrane");
    assert_eq!(names(&data, &by_artist), ["Blue Train", "Giant Steps"]);
}

#[test]
fn compute_indices_year_sort_breaks_ties_by_name_and_honours_dir() {
    let data = GridData::new(vec![
        album(1, "Later", "X", Some(2001)),
        album(2, "Earlier", "X", Some(1999)),
        album(3, "AlsoEarlier", "X", Some(1999)),
    ]);
    let asc = compute_indices(&data, "year", "asc", "");
    assert_eq!(names(&data, &asc), ["AlsoEarlier", "Earlier", "Later"]);
    let desc = compute_indices(&data, "year", "desc", "");
    assert_eq!(names(&data, &desc), ["Later", "Earlier", "AlsoEarlier"]);
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
    assert!(!c.matches("x", "year", "asc"));
    assert!(!c.matches("x", "name", "desc"));
}

#[test]
fn compute_album_cover_cap_clamps_and_scales_with_resolution() {
    // A tiny display can't fill many cards — clamps to the floor (32).
    assert_eq!(compute_album_cover_cap(640, 480).get(), 32);
    // A 4K panel shows far more than the ceiling — clamps to the cap (96).
    assert_eq!(compute_album_cover_cap(3840, 2160).get(), 96);
    // A mid-range display lands strictly between the clamps...
    let mid = compute_album_cover_cap(1920, 1080).get();
    assert!(
        mid > 32 && mid < 96,
        "1080p cap {mid} should sit between the clamps"
    );
    // ...and the cap is monotonic in display area.
    let small = compute_album_cover_cap(1280, 720).get();
    let large = compute_album_cover_cap(2560, 1440).get();
    assert!(small <= mid && mid <= large);
}
