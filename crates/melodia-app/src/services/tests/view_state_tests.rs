use std::collections::BTreeMap;

use melodia_core::entities::track_list::SortDir;
use melodia_core::error::AppError;

use super::*;

fn json_err(e: &serde_json::Error) -> AppError {
    AppError::Validation(format!("json error: {e}"))
}

/// Enough keys that a map saving in an order of its own rarely lands back on the same one.
const SIX_VIEW_IDS: [&str; 6] =
    ["tracks", "album_detail", "artist_detail", "genre_detail", "playlist_detail", "favorites"];

/// Every map filled from [`SIX_VIEW_IDS`]. Converted from arrays rather than spelled as a
/// `BTreeMap`, so the fixture builds whatever map type the fields hold.
fn views_across_six_lists() -> ViewStateData {
    let sort = ViewSort { field: "title".to_owned(), dir: SortDir::Desc };
    ViewStateData {
        view_columns: SIX_VIEW_IDS.map(|id| (id.to_owned(), vec!["title".to_owned()])).into(),
        view_column_widths: SIX_VIEW_IDS.map(|id| (id.to_owned(), ColumnWidths::default())).into(),
        view_sort: SIX_VIEW_IDS.map(|id| (id.to_owned(), sort.clone())).into(),
        last_detail_ids: SIX_VIEW_IDS.map(|id| (id.to_owned(), 42)).into(),
        ..ViewStateData::default()
    }
}

#[test]
fn test_view_state_default() {
    let vs = ViewStateData::default();
    assert_eq!(vs.last_nav_index, 3);
    assert!(vs.view_columns.is_empty());
    assert!(vs.view_column_widths.is_empty());
    assert!(vs.view_sort.is_empty());
    assert!(vs.browse_path.is_none());
    assert_eq!(vs.browse_view_mode, 0);
    assert!(vs.last_detail_ids.is_empty());
    assert!(!vs.artist_albums_collapsed);
    assert_eq!(vs.settings_category, 0);
    assert_eq!(vs.favorites_tab, 0);
    assert_eq!(vs.recently_played_tab, 0);
    assert_eq!(vs.my_library_tab, 0);
}

#[test]
fn test_view_state_missing_fields_default() -> Result<(), AppError> {
    // An empty object must deserialize to all-defaults — mirrors a fresh
    // `views.json` or a partial file written by an older client.
    let vs: ViewStateData = serde_json::from_str("{}").map_err(|e| json_err(&e))?;
    assert_eq!(vs.last_nav_index, 3);
    assert!(vs.view_sort.is_empty());
    // Written before either page had tabs — must read back as the first tab,
    // not fail the whole file. Browse's presentation is the same story: a file
    // predating the card view lands on the list.
    assert_eq!(vs.browse_view_mode, 0);
    assert_eq!(vs.settings_category, 0);
    assert_eq!(vs.favorites_tab, 0);
    assert_eq!(vs.recently_played_tab, 0);
    assert_eq!(vs.my_library_tab, 0);
    Ok(())
}

/// A `views.json` from before the Favorites page was tabbed still carries the
/// two collapse flags the strips used. Serde ignores unknown keys by default,
/// so an installed client must upgrade in place and land on the first tab
/// rather than refusing to load its whole view state.
#[test]
fn test_view_state_ignores_the_retired_collapse_flags() -> Result<(), AppError> {
    let legacy = r#"{
        "last_nav_index": 2,
        "favorites_artists_collapsed": true,
        "favorites_most_played_collapsed": true
    }"#;
    let vs: ViewStateData = serde_json::from_str(legacy).map_err(|e| json_err(&e))?;
    assert_eq!(vs.last_nav_index, 2);
    assert_eq!(vs.favorites_tab, 0);
    Ok(())
}

/// A `views.json` from before the marks holds columns and no `view_marks`, and has to keep the
/// columns rather than fail the whole file.
#[test]
fn a_views_file_from_before_the_marks_keeps_its_columns() -> Result<(), AppError> {
    let legacy = r#"{"view_columns": {"tracks": ["artist", "year"]}}"#;

    let vs: ViewStateData = serde_json::from_str(legacy).map_err(|e| json_err(&e))?;

    let columns = vec!["artist".to_owned(), "year".to_owned()];
    assert_eq!((vs.view_columns.get("tracks"), vs.view_marks.len()), (Some(&columns), 0));
    Ok(())
}

/// The five tabs' numbers overlap the categories' (3 was Services, it is Appearance now), so a
/// `views.json` from before the panel has to open Settings on the first category rather than on
/// whichever one its old tab's number lands on. An alias for the old key is the fold this refuses.
#[test]
fn the_retired_settings_tab_is_not_read_as_a_category() -> Result<(), AppError> {
    let legacy = r#"{"settings_tab": 3}"#;

    let vs: ViewStateData = serde_json::from_str(legacy).map_err(|e| json_err(&e))?;

    assert_eq!(vs.settings_category, 0);
    Ok(())
}

#[test]
fn test_view_state_roundtrip() -> Result<(), AppError> {
    let vs = ViewStateData {
        browse_path: Some("/music/rock".to_owned()),
        browse_view_mode: 1,
        last_nav_index: 5,
        artist_albums_collapsed: true,
        last_detail_ids: BTreeMap::from([("album_detail".to_owned(), 42)]),
        settings_category: 3,
        recently_played_tab: 1,
        my_library_tab: 2,
        ..ViewStateData::default()
    };
    let json = serde_json::to_string(&vs).map_err(|e| json_err(&e))?;
    let back: ViewStateData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(back.browse_path.as_deref(), Some("/music/rock"));
    assert_eq!(back.browse_view_mode, 1);
    assert_eq!(back.last_nav_index, 5);
    assert!(back.artist_albums_collapsed);
    assert_eq!(back.last_detail_ids.get("album_detail").copied(), Some(42));
    assert_eq!(back.settings_category, 3);
    assert_eq!(back.recently_played_tab, 1);
    assert_eq!(back.my_library_tab, 2);
    Ok(())
}

/// Every quit rewrites `views.json`, so a quit that changed nothing has to leave it as it was.
#[test]
fn a_save_that_changes_nothing_leaves_views_json_as_it_was() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    write_view_state(&paths, &views_across_six_lists())?;
    let before = std::fs::read_to_string(&paths.view_state_path)?;

    mutate_view_state(&paths, |_| {})?;

    assert_eq!(
        std::fs::read_to_string(&paths.view_state_path)?,
        before,
        "a save with nothing changed reordered the file"
    );
    Ok(())
}

#[test]
fn test_view_sort_in_view_state() -> Result<(), AppError> {
    let json = r#"{"view_sort": {"tracks": {"field": "title", "dir": "desc"}, "albums": {"field": "year", "dir": "asc"}}}"#;
    let vs: ViewStateData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert_eq!(vs.view_sort.len(), 2);
    let tracks_sort = vs
        .view_sort
        .get("tracks")
        .ok_or_else(|| AppError::Validation("missing tracks sort".into()))?;
    assert_eq!(tracks_sort.field, "title");
    assert!(matches!(tracks_sort.dir, SortDir::Desc));
    let albums_sort = vs
        .view_sort
        .get("albums")
        .ok_or_else(|| AppError::Validation("missing albums sort".into()))?;
    assert_eq!(albums_sort.field, "year");
    assert!(matches!(albums_sort.dir, SortDir::Asc));
    Ok(())
}

// Both halves of the nav-index bound now sit in `crates/melodia/tests/cross_tier.rs`: the write
// clamp is this crate's and the read guard the binary's, and no crate can `include_str!` both.
// What stays here is the round trip itself, which needs neither.

/// A `views.json` naming Radio comes back naming Radio.
#[test]
fn the_top_of_the_range_survives_a_views_json_round_trip() -> Result<(), AppError> {
    let vs =
        ViewStateData { last_nav_index: MAX_NAV_INDEX, radio_tab: 2, ..ViewStateData::default() };
    let json = serde_json::to_string(&vs).map_err(|e| json_err(&e))?;
    let back: ViewStateData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(back.last_nav_index, MAX_NAV_INDEX);
    assert_eq!(back.radio_tab, 2);
    Ok(())
}
