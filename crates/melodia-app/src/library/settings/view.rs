//! View-section setters: locale, overflow-menu buttons, the corner radius snap, and per-view state
//! in `views.json`.

use std::collections::BTreeMap;

use crate::services;
use melodia_core::config::Paths;
use melodia_core::entities::locale::SUPPORTED_LOCALES;
use melodia_core::error::AppError;

/// Validated against [`SUPPORTED_LOCALES`], so a malformed UI write can't pin `settings.json` to a
/// code with no bundled `.po`.
pub fn set_locale(paths: &Paths, code: String) -> Result<(), AppError> {
    if !SUPPORTED_LOCALES.contains(&code.as_str()) {
        return Err(AppError::validation_msg(format!("Unsupported locale: {code}")));
    }
    services::settings::mutate_settings(paths, move |settings| {
        settings.locale = code;
    })
}

/// Removed, then re-appended iff `overflow_on`, so callers needn't know whether it was present.
pub fn set_overflow_button(paths: &Paths, id: String, overflow_on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |s| {
        s.overflow_buttons.retain(|x| x != &id);
        if overflow_on {
            s.overflow_buttons.push(id);
        }
    })
}

/// Snap a radius to the nearest chip preset, so a hand-edited or stranded value still selects a
/// chip: `px-to-preset(snap_to_preset(x))` never answers `-1`. Ties round toward the larger preset
/// (3 → 6, 7 → 8, 9 → 10), so off-preset values lean rounder rather than squarer.
#[must_use]
pub fn snap_to_preset(px: u32) -> u32 {
    match px {
        0..=2 => 0,
        3..=6 => 6,
        7..=8 => 8,
        9..=12 => 10,
        _ => 15,
    }
}

/// `None` lands the next launch on the library folder list.
pub fn set_browse_path(paths: &Paths, path: Option<String>) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.browse_path = path;
    })
}

pub fn set_browse_view_mode(paths: &Paths, mode: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.browse_view_mode = mode;
    })
}

/// Clamped to `globals/nav.slint`'s range, so a bad write can't pin the app to an unselectable tab.
pub fn set_last_nav_index(paths: &Paths, idx: i32) -> Result<(), AppError> {
    let clamped = idx.clamp(0, services::view_state::MAX_NAV_INDEX);
    services::view_state::mutate_view_state(paths, move |s| {
        s.last_nav_index = clamped;
    })
}

/// `None` removes the entry, so the tab lands on its root. Keyed by the view-id strings
/// `view_columns` and `view_sort` use, so one map covers every drill-in view.
pub fn set_last_detail_id(paths: &Paths, view_id: &str, id: Option<i64>) -> Result<(), AppError> {
    let key = view_id.to_owned();
    services::view_state::mutate_view_state(paths, move |s| match id {
        Some(v) => {
            s.last_detail_ids.insert(key, v);
        }
        None => {
            s.last_detail_ids.remove(&key);
        }
    })
}

pub fn set_artist_albums_collapsed(paths: &Paths, collapsed: bool) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.artist_albums_collapsed = collapsed;
    })
}

pub fn set_settings_category(paths: &Paths, category: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.settings_category = category;
    })
}

pub fn set_favorites_tab(paths: &Paths, tab: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.favorites_tab = tab;
    })
}

pub fn set_recently_played_tab(paths: &Paths, tab: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.recently_played_tab = tab;
    })
}

pub fn set_my_library_tab(paths: &Paths, tab: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.my_library_tab = tab;
    })
}

pub fn set_radio_tab(paths: &Paths, tab: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.radio_tab = tab;
    })
}

/// `columns` is the visible ids, in display order.
pub fn update_view_columns(
    paths: &Paths,
    view_id: String,
    columns: Vec<String>,
) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |vs| {
        vs.view_columns.insert(view_id, columns);
    })
}

pub fn update_view_marks(
    paths: &Paths,
    view_id: String,
    marks: BTreeMap<String, bool>,
) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |vs| {
        vs.view_marks.insert(view_id, marks);
    })
}

pub fn set_view_sort(
    paths: &Paths,
    view_id: String,
    sort: services::settings::ViewSort,
) -> Result<(), AppError> {
    services::view_state::mutate_view_state(paths, move |s| {
        s.view_sort.insert(view_id, sort);
    })
}

/// `None` when the view never persisted one or `views.json` won't load, the caller's default
/// keeping the view usable either way.
#[must_use]
pub fn get_view_sort(paths: &Paths, view_id: &str) -> Option<services::settings::ViewSort> {
    services::view_state::read_view_state(paths)
        .ok()
        .and_then(|s| s.view_sort.get(view_id).cloned())
}

#[cfg(test)]
#[path = "tests/view_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/view_writers_tests.rs"]
mod writer_tests;
