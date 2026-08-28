use std::collections::HashMap;

use crate::error::AppError;
use crate::services::settings::SortDir;

use super::*;

fn json_err(e: &serde_json::Error) -> AppError {
    AppError::Validation(format!("json error: {e}"))
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
    assert_eq!(vs.settings_tab, 0);
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
    assert_eq!(vs.settings_tab, 0);
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

#[test]
fn test_view_state_roundtrip() -> Result<(), AppError> {
    let vs = ViewStateData {
        browse_path: Some("/music/rock".to_owned()),
        browse_view_mode: 1,
        last_nav_index: 5,
        artist_albums_collapsed: true,
        last_detail_ids: HashMap::from([("album_detail".to_owned(), 42)]),
        settings_tab: 3,
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
    assert_eq!(back.settings_tab, 3);
    assert_eq!(back.recently_played_tab, 1);
    assert_eq!(back.my_library_tab, 2);
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

/// **The persisted nav index has to survive a round trip at the top of its range**, and until
/// Phase 4 of the radio work it did not: `set_last_nav_index` clamped writes to `0..=9` and
/// `install_views` guarded reads with the same literal, so a Radio index was rewritten as Settings
/// on the way out *and* dropped on the way in. Neither half is visible from the other, which is
/// why both now read [`MAX_NAV_INDEX`] and why this pins the bound against the section that
/// actually sits at the top of it.
#[test]
fn the_nav_bound_reaches_the_highest_section_that_routes() {
    assert_eq!(
        MAX_NAV_INDEX,
        crate::ui::radio::NAV_RADIO,
        "Radio is the highest index `nav.slint` routes, so the bound is its index — a section \
         added above it moves both"
    );
}

/// Both ends of that round trip must take the bound from [`MAX_NAV_INDEX`] rather than restate it.
///
/// A source read because the write needs an `AppState` and the read an `AppWindow`, and because
/// what failed before was not the arithmetic but the *literal*: two sites agreeing on `9` for
/// reasons neither could see.
#[test]
fn both_ends_of_the_round_trip_take_the_bound_from_one_const() {
    const WRITE: &str = include_str!("../../library/settings/view.rs");
    const READ: &str = include_str!("../../boot/ui_setup.rs");

    let clamp = crate::test_support::strip_line_comments(WRITE)
        .split_once("pub fn set_last_nav_index")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or(String::new(), |(body, _)| body.to_owned());
    assert!(!clamp.is_empty(), "`set_last_nav_index` moved, so this pin reads nothing");
    assert!(
        clamp.contains("view_state::MAX_NAV_INDEX"),
        "the write clamp must bound against `MAX_NAV_INDEX`, never a literal"
    );

    let read = crate::test_support::strip_line_comments(READ);
    assert!(
        read.contains("(0..=services::view_state::MAX_NAV_INDEX).contains("),
        "`install_views` must guard the persisted index against the same const the write clamps to"
    );
}

/// A `views.json` naming Radio comes back naming Radio.
#[test]
fn the_top_of_the_range_survives_a_views_json_round_trip() -> Result<(), AppError> {
    let vs = ViewStateData {
        last_nav_index: MAX_NAV_INDEX,
        radio_tab: 2,
        ..ViewStateData::default()
    };
    let json = serde_json::to_string(&vs).map_err(|e| json_err(&e))?;
    let back: ViewStateData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(back.last_nav_index, MAX_NAV_INDEX);
    assert_eq!(back.radio_tab, 2);
    Ok(())
}
