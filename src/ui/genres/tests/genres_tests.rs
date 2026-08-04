use super::*;

/// Minimal `GenreStats` builder — only the fields the grid filter / sort
/// read matter.
fn genre(id: i64, name: &str, track_count: i32, total_duration_ms: i64) -> GenreStats {
    GenreStats {
        id,
        name: name.to_string(),
        track_count,
        total_duration_ms,
    }
}

fn names(data: &GridData, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|&i| data.genres[i].name.clone()).collect()
}

#[test]
fn grid_data_precomputes_the_lowercased_sort_key() {
    let data = GridData::new(vec![genre(1, "Post-Rock", 4, 1_200_000)]);
    assert_eq!(data.keys.len(), 1);
    assert_eq!(data.keys[0].name_lc, "post-rock");
}

#[test]
fn compute_indices_with_empty_filter_keeps_all_genres() {
    let data = GridData::new(vec![
        genre(1, "Alpha", 1, 0),
        genre(2, "Bravo", 1, 0),
    ]);
    let idx = compute_indices(&data, "name", "asc", "");
    assert_eq!(names(&data, &idx), ["Alpha", "Bravo"]);
}

#[test]
fn compute_indices_filter_matches_name_case_insensitively() {
    let data = GridData::new(vec![
        genre(1, "Jazz", 1, 0),
        genre(2, "Smooth Jazz", 1, 0),
        genre(3, "Blues", 1, 0),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "JAZZ");
    assert_eq!(names(&data, &by_name), ["Jazz", "Smooth Jazz"]);
}

#[test]
fn compute_indices_filter_ignores_accents_the_way_the_search_view_does() {
    // Genre is the name the Search view reaches least well — its only
    // entity-side arm is an unfolded `name LIKE`, so an accented genre never
    // surfaces as a genre result there. This grid is where an ASCII query has
    // to find one.
    let data = GridData::new(vec![
        genre(1, "Musique concrète", 1, 0),
        genre(2, "Blues", 1, 0),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "concrete");
    assert_eq!(names(&data, &by_name), ["Musique concrète"]);
}

#[test]
fn compute_indices_track_count_sort_breaks_ties_by_name_and_honours_dir() {
    let data = GridData::new(vec![
        genre(1, "Larger", 10, 0),
        genre(2, "Bcount", 5, 0),
        genre(3, "Acount", 5, 0),
    ]);
    let asc = compute_indices(&data, "track_count", "asc", "");
    // Ties between "Acount" and "Bcount" at count 5 break by name; "Larger" wins by count.
    assert_eq!(names(&data, &asc), ["Acount", "Bcount", "Larger"]);
    let desc = compute_indices(&data, "track_count", "desc", "");
    assert_eq!(names(&data, &desc), ["Larger", "Bcount", "Acount"]);
}

#[test]
fn compute_indices_duration_sort_breaks_ties_by_name_and_honours_dir() {
    let data = GridData::new(vec![
        genre(1, "Long", 1, 600_000),
        genre(2, "B-Short", 1, 100_000),
        genre(3, "A-Short", 1, 100_000),
    ]);
    let asc = compute_indices(&data, "duration", "asc", "");
    assert_eq!(names(&data, &asc), ["A-Short", "B-Short", "Long"]);
    let desc = compute_indices(&data, "duration", "desc", "");
    assert_eq!(names(&data, &desc), ["Long", "B-Short", "A-Short"]);
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
