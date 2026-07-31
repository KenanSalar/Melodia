//! View-section setters: locale, overflow-menu buttons, snap-to-preset
//! corner radius helper, and per-view column visibility.

use crate::error::AppError;
use crate::services;
use crate::state::AppState;

/// Persist the active locale. Validates against
/// [`services::settings::SUPPORTED_LOCALES`] before the disk write so a
/// malformed UI write (or a stale `language-changed` index from a race
/// against a hot-reload of the list) can't pin `settings.json` to a code
/// that has no bundled `.po`. The runtime application
/// (`slint::select_bundled_translation`) happens synchronously in
/// `src/ui/locale.rs::wire_language_changed` *before* this async persist —
/// the UI re-renders immediately; this write only carries the choice
/// across the next process boot.
pub fn set_locale(state: &AppState, code: String) -> Result<(), AppError> {
    if !services::settings::SUPPORTED_LOCALES.contains(&code.as_str()) {
        return Err(AppError::Validation(format!(
            "Unsupported locale: {code}"
        )));
    }
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.locale = code;
    })
}

/// Persist a single Overflow Menu Buttons toggle. Mirrors the Tauri
/// frontend's `updateSetting("overflow_buttons", current)` write: the
/// id is removed unconditionally first (idempotent dedupe), then
/// re-appended iff `overflow_on` is true. Callers don't need to know
/// whether the id was already present.
pub fn set_overflow_button(
    state: &AppState,
    id: String,
    overflow_on: bool,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.overflow_buttons.retain(|x| x != &id);
        if overflow_on {
            s.overflow_buttons.push(id);
        }
    })
}

/// Snap an arbitrary radius to the nearest chip preset (0, 6, 8, 10, 15).
/// The chip group is the only legitimate input path now — the manual
/// stepper was removed — but `settings.json` is user-editable and a
/// future preset addition could leave older values stranded between
/// chips. This snap keeps the chip selection and the painted radius in
/// agreement: `px-to-preset(snap_to_preset(x))` always returns a valid
/// chip index, never `-1`.
///
/// Tie-breaking rounds toward the larger preset (3 → 6, 7 → 8, 9 → 10)
/// so off-preset values lean rounder rather than squarer — matches the
/// "feel more native" framing of the OS-aware defaults. Caller is
/// expected to clamp to `MAX_CORNER_RADIUS` first; this fn maps any
/// value > 12 to 15.
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

/// Persist the Browse view's "current path" so the next launch opens at
/// the same folder. Passing `None` clears the field — the next launch
/// will land at the root (library folder list). No runtime kick: the
/// Slint property is already updated synchronously by the open-folder
/// callback before this write runs.
pub fn set_browse_path(state: &AppState, path: Option<String>) -> Result<(), AppError> {
    services::view_state::mutate_view_state(&state.paths, move |s| {
        s.browse_path = path;
    })
}

/// Persist the active sidebar tab so the next launch reopens on the same
/// section. Indices follow the `Nav` global comment in `melodia-ui/ui/globals/nav.slint`.
/// Out-of-range writes (negative / > 9) are clamped to the valid range so a
/// corrupt write can't pin the app to an unselectable tab.
pub fn set_last_nav_index(state: &AppState, idx: i32) -> Result<(), AppError> {
    let clamped = idx.clamp(0, 9);
    services::view_state::mutate_view_state(&state.paths, move |s| {
        s.last_nav_index = clamped;
    })
}

/// Persist (or clear) the detail-view id open for `view_id` in
/// `views.json`'s `last_detail_ids`. `Some(id)` means the next launch on that
/// tab reopens this detail page; `None` removes the entry so the tab
/// lands on its root (grid / list). The map is keyed by the same view-id
/// strings as `view_columns` / `view_sort` (see
/// `src/ui/track_list_view.rs::view_id`), so one call covers Album,
/// Artist, Genre, Playlist detail (and any future drill-in views)
/// without a new field per view. No runtime kick: the Slint property is
/// already up-to-date before this write runs.
pub fn set_last_detail_id(
    state: &AppState,
    view_id: &str,
    id: Option<i64>,
) -> Result<(), AppError> {
    let key = view_id.to_owned();
    services::view_state::mutate_view_state(&state.paths, move |s| match id {
        Some(v) => {
            s.last_detail_ids.insert(key, v);
        }
        None => {
            s.last_detail_ids.remove(&key);
        }
    })
}

/// Persist the Artist Detail "Albums" sub-section collapsed flag so the
/// next launch reopens the sub-section in the same state. No runtime
/// kick: the Slint property is updated synchronously by the toggle
/// callback before this write runs.
pub fn set_artist_albums_collapsed(
    state: &AppState,
    collapsed: bool,
) -> Result<(), AppError> {
    services::view_state::mutate_view_state(&state.paths, move |s| {
        s.artist_albums_collapsed = collapsed;
    })
}

/// Persist the Settings page's active tab so re-entering the page lands
/// where the user left it. Same no-kick shape as the collapse flag above:
/// `SettingsPage.tab-idx` is two-way bound to the tab bar, so the Slint side
/// is already correct by the time this write runs.
pub fn set_settings_tab(state: &AppState, tab: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(&state.paths, move |s| {
        s.settings_tab = tab;
    })
}

/// Persist the Favorites page's active tab. Same contract as
/// [`set_settings_tab`] — `Favorites.tab-idx` is two-way bound to its tab
/// bar, so this write is pure catch-up.
pub fn set_favorites_tab(state: &AppState, tab: i32) -> Result<(), AppError> {
    services::view_state::mutate_view_state(&state.paths, move |s| {
        s.favorites_tab = tab;
    })
}

/// Persist the visible-column list for `view_id` (e.g. "tracks") into
/// `views.json`'s `view_columns`. Full file rewrite, matching how the Tauri
/// app handled this. `columns` is the list of CURRENTLY VISIBLE column
/// IDs in display order. Serialized via `mutate_view_state` to avoid the
/// read-then-write race when multiple views save concurrently.
pub fn update_view_columns(
    state: &AppState,
    view_id: String,
    columns: Vec<String>,
) -> Result<(), AppError> {
    services::view_state::mutate_view_state(&state.paths, move |vs| {
        vs.view_columns.insert(view_id, columns);
    })
}

/// Persist the active sort (field + direction) for `view_id` into
/// `views.json`'s `view_sort`. Keyed by the same view-id strings as
/// `view_columns` / `last_detail_ids` so one map covers Tracks, Albums,
/// Artists, Genres, Playlists, Favorites without a new top-level field
/// per view. No runtime kick: the Slint property is updated synchronously
/// by the sort-pill callback before this write runs.
pub fn set_view_sort(
    state: &AppState,
    view_id: String,
    sort: services::settings::ViewSort,
) -> Result<(), AppError> {
    services::view_state::mutate_view_state(&state.paths, move |s| {
        s.view_sort.insert(view_id, sort);
    })
}

/// Read the persisted sort for `view_id`, if any. Cheap clone — `ViewSort`
/// is two small owned fields. Returns `None` when the view has never
/// persisted a sort (caller picks its own default) or when `views.json`
/// fails to load (a fresh launch or a corrupt write — the caller's
/// default keeps the view usable either way).
#[must_use]
pub fn get_view_sort(
    state: &AppState,
    view_id: &str,
) -> Option<services::settings::ViewSort> {
    services::view_state::read_view_state(&state.paths)
        .ok()
        .and_then(|s| s.view_sort.get(view_id).cloned())
}

#[cfg(test)]
#[path = "tests/view_tests.rs"]
mod tests;
