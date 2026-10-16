//! The cached result set a favourite or rating edit has to reach, since "Show all", a re-sort and a
//! reapply all repaint from it rather than from the rows on screen; and the models the global is
//! handed at install, which every later write finds by downcasting back.

use std::collections::BTreeSet;

use melodia_core::entities::search::SearchResults;
use melodia_testkit::{block_after, strip_line_comments};

use super::*;

const GLOBAL: &str = include_str!("../../../../../melodia-ui/ui/globals/search.slint");
const MOD: &str = include_str!("../mod.rs");

fn row(id: i64) -> RsTrackListRow {
    RsTrackListRow {
        id,
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

/// Every list is written through `replace_rows`, the recent searches included, and its downcast
/// misses on an array left unbound: the list stays empty, and the only report is a log line per
/// write.
#[test]
fn every_array_the_search_global_declares_is_handed_a_model() {
    let declared: BTreeSet<String> = strip_line_comments(GLOBAL)
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("in-out property <["))
        .filter_map(|rest| rest.split_once("]>"))
        .filter_map(|(_, rest)| rest.split_once(';'))
        .map(|(name, _)| name.trim().replace('-', "_"))
        .collect();
    let install = block_after(MOD, "fn install_models(");
    let installed: BTreeSet<String> = install
        .split("g.set_")
        .skip(1)
        .filter_map(|call| call.split_once('('))
        .map(|(name, _)| name.to_owned())
        .collect();

    assert!(!declared.is_empty(), "the walk over `globals/search.slint` found no array");
    assert_eq!(installed, declared, "`install_models` and the arrays `Search` declares disagree");
    assert!(
        MOD.contains("install_models(cx.app);"),
        "`install` must call `install_models` before anything can write a list"
    );
}
