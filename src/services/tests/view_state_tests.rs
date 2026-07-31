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
    assert!(vs.last_detail_ids.is_empty());
    assert!(!vs.artist_albums_collapsed);
    assert_eq!(vs.settings_tab, 0);
    assert_eq!(vs.favorites_tab, 0);
}

#[test]
fn test_view_state_missing_fields_default() -> Result<(), AppError> {
    // An empty object must deserialize to all-defaults — mirrors a fresh
    // `views.json` or a partial file written by an older client.
    let vs: ViewStateData = serde_json::from_str("{}").map_err(|e| json_err(&e))?;
    assert_eq!(vs.last_nav_index, 3);
    assert!(vs.view_sort.is_empty());
    // Written before either page had tabs — must read back as the first tab,
    // not fail the whole file.
    assert_eq!(vs.settings_tab, 0);
    assert_eq!(vs.favorites_tab, 0);
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
        last_nav_index: 5,
        artist_albums_collapsed: true,
        last_detail_ids: HashMap::from([("album_detail".to_owned(), 42)]),
        settings_tab: 3,
        ..ViewStateData::default()
    };
    let json = serde_json::to_string(&vs).map_err(|e| json_err(&e))?;
    let back: ViewStateData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(back.browse_path.as_deref(), Some("/music/rock"));
    assert_eq!(back.last_nav_index, 5);
    assert!(back.artist_albums_collapsed);
    assert_eq!(back.last_detail_ids.get("album_detail").copied(), Some(42));
    assert_eq!(back.settings_tab, 3);
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
