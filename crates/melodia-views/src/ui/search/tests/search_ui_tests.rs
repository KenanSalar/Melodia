//! The cached result set a favourite or rating edit has to reach, since "Show all", a re-sort and a
//! reapply all repaint from it rather than from the rows on screen.

use melodia_core::entities::search::SearchResults;

use super::*;

fn row(id: i64) -> RsTrackListRow {
    RsTrackListRow {
        id,
        file_path: String::new(),
        file_name: String::new(),
        title: format!("Track {id}"),
        artist: None,
        album_artist: None,
        album: None,
        genre: None,
        track_number: None,
        disc_number: None,
        year: None,
        duration_ms: 0,
        artwork_path: None,
        is_favorite: false,
        rating: 0,
        album_id: None,
        artist_id: None,
        genre_id: None,
        date_added: String::new(),
        sort_key: None,
    }
}

fn searched(tracks: Vec<RsTrackListRow>) -> SearchUi {
    let ui = SearchUi::new(Arc::new(CoverThumbs::new()));
    *ui.inner.last_results.lock() =
        Some(SearchResults { tracks, albums: Vec::new(), artists: Vec::new(), genres: Vec::new() });
    ui
}

fn cached(ui: &SearchUi) -> Vec<(bool, i32)> {
    ui.inner
        .last_results
        .lock()
        .as_ref()
        .map(|results| results.tracks.iter().map(|t| (t.is_favorite, t.rating)).collect())
        .unwrap_or_default()
}

/// Left unpatched, the next repaint from the cache put the old heart back on the row.
#[test]
fn a_favourite_flip_patches_the_cached_row_it_names_and_no_other() {
    let ui = searched(vec![row(1), row(2), row(3)]);

    ui.flip_favorite(2, true);

    assert_eq!(cached(&ui), [(false, 0), (true, 0), (false, 0)]);
}

#[test]
fn a_rating_flip_patches_the_rating_and_leaves_the_favourite_alone() {
    let ui = searched(vec![RsTrackListRow { is_favorite: true, ..row(1) }, row(2)]);

    ui.flip_rating(1, 4);

    assert_eq!(cached(&ui), [(true, 4), (false, 0)]);
}
