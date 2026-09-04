//! Accent dot-grid wiring + the swatch helpers shared with
//! [`super::apply_and_seed`]. Owns the `MATERIAL_YOU_ACCENT_ID` resolution
//! pathway: when MY is active the live dynamic primary sits at index 0 and
//! the 8 static accents shift right; when MY isn't active those helpers
//! collapse to the static-only grid.

use std::sync::Arc;

use parking_lot::RwLock;
use slint::{Brush, ComponentHandle, SharedString};

use super::theme_apply::{accent_brushes, brush};
use melodia_app::state::{AppState, Signal};
use melodia_core::themes::{self, MATERIAL_YOU_ACCENT_ID, SystemColorState, ThemeDef};
use melodia_ui::{AppWindow, Settings};

use super::{PersistedAccent, persist, registry_get, usize_from};

/// True when the active theme is Material 3 *and* the coordinator has
/// produced a dynamic palette + accent for the current artwork. Drives
/// `Settings.material-you-active`, the Settings UI's "prepend the live
/// primary as the 9th accent dot" decision, and `accent_id` resolution
/// inside [`super::apply_and_seed`].
pub(super) fn material_you_active(theme_id: &str, system: &SystemColorState) -> bool {
    theme_id == "material3" && system.material_you.is_some()
}

/// Build the accent swatch list for the dot grid plus matching tooltip
/// labels. When Material You is active for the current `theme` +
/// `system`, a 9th brush rendered in the dynamic primary is prepended
/// with label `"Material You"`; otherwise it's the existing 8-dot list
/// from `theme_apply::accent_brushes` paired with each `AccentDef.name`.
/// Returns the swatches, the labels, and a bool indicating whether the
/// Material You dot was prepended.
pub(super) fn accent_swatches_with_my(
    theme: &ThemeDef,
    variant_id: &str,
    system: &SystemColorState,
) -> (Vec<Brush>, Vec<SharedString>, bool) {
    let my_active = material_you_active(theme.id, system);
    let mut brushes = accent_brushes(theme, variant_id);
    let mut labels: Vec<SharedString> =
        theme.accents.iter().map(|a| SharedString::from(a.name)).collect();
    if my_active && let Some((_, dyn_accent)) = &system.material_you {
        brushes.insert(0, brush(*dyn_accent));
        labels.insert(0, SharedString::from("Material You"));
    }
    (brushes, labels, my_active)
}

/// Map a persisted `accent_id` to its index in the visible swatch grid.
/// When Material You is active the live `MATERIAL_YOU_ACCENT_ID` id
/// sits at index 0 and the 8 static accents shift one slot to the right.
/// The caller is expected to have already resolved the effective accent
/// via [`effective_accent_id`] when MY is selected but inactive — this
/// function does no fallback of its own.
pub(super) fn accent_idx_in_grid(theme: &ThemeDef, accent_id: &str, my_active: bool) -> usize {
    if my_active && accent_id == MATERIAL_YOU_ACCENT_ID {
        return 0;
    }
    let static_idx = themes::accent_index(theme, accent_id);
    static_idx + usize::from(my_active)
}

/// Resolve the accent id we actually want to render and paint with.
/// When the persisted `accent_id` is `MATERIAL_YOU_ACCENT_ID` but MY
/// isn't currently active (theme isn't material3, style is None, or no
/// dynamic palette has been generated yet because there's no artwork),
/// fall back to the user's last static accent — and finally to the
/// theme's hard default if no static was ever picked.
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

/// Reverse of [`accent_idx_in_grid`]: resolve a clicked grid index to a
/// concrete accent id. Index 0 → `MATERIAL_YOU_ACCENT_ID` when active;
/// otherwise the static accent at `idx - my_active as usize`.
pub(super) fn accent_id_from_grid_idx(
    theme: &ThemeDef,
    idx: usize,
    my_active: bool,
) -> Option<&'static str> {
    if my_active && idx == 0 {
        return Some(MATERIAL_YOU_ACCENT_ID);
    }
    let offset = usize::from(my_active);
    theme.accents.get(idx.saturating_sub(offset)).map(|a| a.id)
}

pub(super) fn wire_accent_changed(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    _kick: Signal,
    persisted_accent: PersistedAccent,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_accent_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let theme_idx = g.get_theme_idx();
        let variant_idx = g.get_variant_idx();
        let Some(theme) = registry_get(theme_idx) else {
            return;
        };

        let i = usize_from(variant_idx);
        let variant_id: &str = if theme.supports_system_mode && i == theme.variants.len() {
            themes::SYSTEM_VARIANT_ID
        } else if let Some(v) = theme.variants.get(i) {
            v.id
        } else {
            return;
        };

        let snapshot = os_state.read().clone();
        let my_active = material_you_active(theme.id, &snapshot);
        let Some(accent_id) = accent_id_from_grid_idx(theme, usize_from(idx), my_active) else {
            return;
        };
        g.set_accent_idx(idx);
        super::apply_palette(&ui, theme.id, variant_id, accent_id, &snapshot);
        // Update the synchronous accent shadow before the async write
        // — a sibling `wire_variant_changed` that fires before the disk
        // commit will then read the new accent (not the previous one).
        accent_id.clone_into(&mut persisted_accent.lock());
        persist(&s, theme.id, variant_id, accent_id);
        // Accent picks don't regenerate the M3 surfaces — kick is not
        // strictly required, but cheap and keeps the coordinator's
        // last-applied tracking honest.
    });
}
