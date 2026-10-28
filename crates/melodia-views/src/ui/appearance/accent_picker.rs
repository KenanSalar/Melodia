//! Accent dot-grid wiring, the Material You switch above it, and the swatch helpers shared with
//! [`super::apply_and_seed`] and the window border's grid, which shows the same swatches.
//!
//! **Material You is a switch, not a swatch.** `MATERIAL_YOU_ACCENT_ID` follows the artwork rather
//! than naming a colour, so while it is the persisted accent the grid checks nothing, and a pick
//! in the grid turns it off.

use std::sync::Arc;

use parking_lot::RwLock;
use slint::{Brush, ComponentHandle, SharedString};

use super::theme_apply::accent_brushes;
use melodia_app::state::AppState;
use melodia_core::themes::material3::MATERIAL3;
use melodia_core::themes::{MATERIAL_YOU_ACCENT_ID, SystemColorState, ThemeDef};
use melodia_ui::{AppWindow, Settings};

use super::{
    PersistedAccent, persist, read_last_static_accent, registry_get, usize_from, variant_id_at,
};

/// Whether the theme is Material 3 and the coordinator has a palette for the current artwork.
pub(super) fn material_you_active(theme_id: &str, system: &SystemColorState) -> bool {
    theme_id == MATERIAL3.id && system.material_you.is_some()
}

/// The theme's accents shaded for `variant_id`, with the names their tooltips carry.
pub(super) fn accent_swatches(
    theme: &ThemeDef,
    variant_id: &str,
) -> (Vec<Brush>, Vec<SharedString>) {
    let labels = theme.accents.iter().map(|a| SharedString::from(a.name)).collect();
    (accent_brushes(theme, variant_id), labels)
}

/// The grid slot showing `accent_id`, `None` for an id that names no swatch: Material You's, the
/// window border's System, or one the theme doesn't ship.
pub(super) fn accent_slot(theme: &ThemeDef, accent_id: &str) -> Option<usize> {
    theme.accents.iter().position(|a| a.id == accent_id)
}

/// The accent a clicked slot stands for, `None` past the grid's end.
pub(super) fn accent_at(theme: &ThemeDef, slot: usize) -> Option<&'static str> {
    theme.accents.get(slot).map(|a| a.id)
}

/// `slot` as a grid's `selected-index`, where -1 checks no swatch.
pub(super) fn slot_index(slot: Option<usize>) -> i32 {
    slot.map_or(-1, super::apply_and_seed_to_i32)
}

/// Seat `accent_id` in the grid and the Material You switch, which never both read as picked.
pub(super) fn seat_accent(g: &Settings<'_>, theme: &ThemeDef, accent_id: &str) {
    g.set_accent_follows_artwork(accent_id == MATERIAL_YOU_ACCENT_ID);
    g.set_accent_idx(slot_index(accent_slot(theme, accent_id)));
}

/// The accent to paint: a Material You accent with Material You inactive falls back to the last
/// one picked by hand, then to the theme's default.
pub(super) fn effective_accent_id<'a>(
    theme: &'a ThemeDef,
    accent_id: &'a str,
    my_active: bool,
    last_static_accent: Option<&'a str>,
) -> &'a str {
    if accent_id == MATERIAL_YOU_ACCENT_ID && !my_active {
        last_static_accent.unwrap_or(theme.default_accent)
    } else {
        accent_id
    }
}

pub(super) fn wire_accent_changed(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    persisted_accent: PersistedAccent,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_accent_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let Some(theme) = registry_get(g.get_theme_idx()) else {
            return;
        };
        let Some(variant_id) = variant_id_at(theme, usize_from(g.get_variant_idx())) else {
            return;
        };
        let Some(accent_id) = accent_at(theme, usize_from(idx)) else {
            return;
        };

        let snapshot = os_state.read().clone();
        seat_accent(&g, theme, accent_id);
        super::apply_palette(&ui, theme.id, variant_id, accent_id, &snapshot);
        accent_id.clone_into(&mut persisted_accent.lock());
        // No kick: the coordinator's snapshot doesn't include the accent.
        persist(&s, theme.id, variant_id, accent_id);
    });
}

/// The Material You switch. Off takes back the theme's last accent picked by hand, as clearing the
/// Color Style does.
pub(super) fn wire_follows_artwork_changed(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    persisted_accent: PersistedAccent,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_accent_follows_artwork_changed(move |follows| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let Some(theme) = registry_get(g.get_theme_idx()) else {
            return;
        };
        let Some(variant_id) = variant_id_at(theme, usize_from(g.get_variant_idx())) else {
            return;
        };

        let last_static = read_last_static_accent(&s, theme.id);
        let accent_id = if follows {
            MATERIAL_YOU_ACCENT_ID
        } else {
            last_static.as_deref().unwrap_or(theme.default_accent)
        };

        let snapshot = os_state.read().clone();
        let my_active = material_you_active(theme.id, &snapshot);
        let painted = effective_accent_id(theme, accent_id, my_active, last_static.as_deref());
        seat_accent(&g, theme, accent_id);
        super::apply_palette(&ui, theme.id, variant_id, painted, &snapshot);
        accent_id.clone_into(&mut persisted_accent.lock());
        persist(&s, theme.id, variant_id, accent_id);
    });
}

#[cfg(test)]
#[path = "tests/accent_picker_tests.rs"]
mod tests;
