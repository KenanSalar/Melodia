//! Material You repaint entrypoint + the shared `apply_and_seed`
//! orchestration core called from `install` and after every appearance
//! callback.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::accent_picker::{accent_idx_in_grid, accent_swatches_with_my, effective_accent_id};
use super::{apply_and_seed_to_i32, theme_picker};
use crate::library;
use crate::media::image::material_you::SchemeStyle;
use crate::state::AppState;
use crate::themes::{self, SystemColorState};
use crate::{AppWindow, Settings};

/// Re-read `settings.json` (theme / variant / accent / dynamic colour
/// style) and repaint the palette using the supplied `system` snapshot.
/// Used by the Material You coordinator after it writes a fresh dynamic
/// palette into `system.material_you` — must run on the UI thread.
pub fn repaint_from_settings(ui: &AppWindow, state: &AppState, system: &SystemColorState) {
    let settings = match library::settings::get_settings(state) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("material_you repaint: read settings: {e}");
            return;
        }
    };
    let last_static = settings
        .theme_preferences
        .get(&settings.theme_id)
        .and_then(|p| p.last_static_accent.clone());
    apply_and_seed(
        ui,
        &settings.theme_id,
        &settings.theme_variant,
        &settings.accent_color,
        &settings.dynamic_color_style,
        last_static.as_deref(),
        system,
    );
}

/// Resolve a possibly-stale `(theme_id, variant_id, accent_id, style)`
/// quadruple from `settings.json`, write every Settings global property,
/// and apply the palette. Used at startup, after a theme change, and
/// whenever the OS appearance state ticks while the persisted variant is
/// `"system"`. `style_id` matches the `dynamic_color_style` field —
/// drives only the `Settings.color-style-idx` seed; whether Material You
/// is *actually* active is decided by `system.material_you.is_some()`
/// (the coordinator publishes that asynchronously after extraction).
///
/// `last_static_accent` is the per-theme remembered static accent; when
/// `accent_id == MATERIAL_YOU_ACCENT_ID` and MY isn't active, we paint
/// and highlight that fallback instead of jumping to the theme default.
pub fn apply_and_seed(
    ui: &AppWindow,
    theme_id: &str,
    variant_id: &str,
    accent_id: &str,
    style_id: &str,
    last_static_accent: Option<&str>,
    system: &SystemColorState,
) {
    let theme = themes::get(theme_id);
    let theme_idx = themes::theme_index(theme.id);
    let variant_idx = theme_picker::variant_index_with_system(theme, variant_id);

    let resolved_variant = if variant_id == themes::SYSTEM_VARIANT_ID {
        themes::SYSTEM_VARIANT_ID
    } else {
        super::resolved_variant_id(theme, variant_idx)
    };

    // Accent swatches use the resolved real variant so the dots render
    // in the correct shade even when the user is on "System".
    let accent_variant_for_swatches = if variant_id == themes::SYSTEM_VARIANT_ID {
        theme.resolve_system_variant(&system.theme).id
    } else {
        super::resolved_variant_id(theme, variant_idx)
    };
    let (brushes, labels, my_active) =
        accent_swatches_with_my(theme, accent_variant_for_swatches, system);
    let g_swatches = ui.global::<Settings>();
    g_swatches.set_accent_colors(ModelRc::from(Rc::new(VecModel::from(brushes))));
    g_swatches.set_accent_names(ModelRc::from(Rc::new(VecModel::from(labels))));

    // When the persisted accent is "material_you" but MY isn't active
    // (no artwork / coordinator hasn't run yet / theme isn't material3),
    // paint with the user's last static pick so the surfaces still match
    // an accent the user actually chose.
    let effective_id = effective_accent_id(theme, accent_id, my_active, last_static_accent);
    let accent_idx = accent_idx_in_grid(theme, effective_id, my_active);
    let style_idx = SchemeStyle::all().iter().position(|s| s.as_id() == style_id).unwrap_or(0);

    let g = ui.global::<Settings>();
    g.set_theme_idx(apply_and_seed_to_i32(theme_idx));
    g.set_variant_idx(apply_and_seed_to_i32(variant_idx));
    g.set_accent_idx(apply_and_seed_to_i32(accent_idx));
    g.set_color_style_idx(apply_and_seed_to_i32(style_idx));
    g.set_kde_system_active(theme_picker::kde_system_active(theme.id, variant_id, system));
    g.set_material_you_active(my_active);

    super::apply_palette(ui, theme.id, resolved_variant, effective_id, system);
}
