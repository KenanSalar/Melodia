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
fn grid_data_precomputes_lowercased_sort_keys() {
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
fn compute_indices_filter_matches_year_and_its_decade() {
    // `AlbumStats` carries the year, so a query the Search view answers
    // with a decade narrows this grid too.
    let data = GridData::new(vec![
        album(1, "Loveless", "My Bloody Valentine", Some(1991)),
        album(2, "Nevermind", "Nirvana", Some(1991)),
        album(3, "Kid A", "Radiohead", Some(2000)),
    ]);
    let by_year = compute_indices(&data, "name", "asc", "1991");
    assert_eq!(names(&data, &by_year), ["Loveless", "Nevermind"]);
    let by_decade = compute_indices(&data, "name", "asc", "199");
    assert_eq!(names(&data, &by_decade), ["Loveless", "Nevermind"]);
}

#[test]
fn compute_indices_filter_ignores_accents_the_way_the_search_view_does() {
    let data = GridData::new(vec![
        album(1, "Ágætis byrjun", "Sigur Rós", None),
        album(2, "Kid A", "Radiohead", None),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "agaetis");
    assert!(by_name.is_empty(), "æ is a letter, not an accented a");
    let by_title = compute_indices(&data, "name", "asc", "byrjun");
    assert_eq!(names(&data, &by_title), ["Ágætis byrjun"]);
    let by_artist = compute_indices(&data, "name", "asc", "sigur ros");
    assert_eq!(names(&data, &by_artist), ["Ágætis byrjun"]);
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
