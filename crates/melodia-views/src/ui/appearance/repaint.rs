//! [`apply_and_seed`], the one repaint `install`, every appearance pick and the Material You
//! coordinator go through.

use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::accent_picker::{
    accent_swatches, effective_accent_id, material_you_active, seat_accent,
};
use super::{apply_and_seed_to_i32, theme_picker};
use melodia_app::library;
use melodia_app::services::settings::SettingsData;
use melodia_app::state::AppState;
use melodia_artwork::media::image::material_you::SchemeStyle;
use melodia_core::error::describe;
use melodia_core::themes::{self, SystemColorState};
use melodia_ui::{AppWindow, Settings};

/// Repaints the persisted pick against `system`, for the Material You coordinator once it has
/// written a fresh palette into `system.material_you`. UI thread.
pub fn repaint_from_settings(ui: &AppWindow, state: &AppState, system: &SystemColorState) {
    match library::settings::get_settings(&state.paths) {
        Ok(settings) => apply_settings(ui, &settings, system),
        Err(e) => log::warn!("material_you repaint: read settings: {}", describe(&e)),
    }
}

/// [`apply_and_seed`] with every argument taken from the persisted `settings`.
pub(super) fn apply_settings(ui: &AppWindow, settings: &SettingsData, system: &SystemColorState) {
    apply_and_seed(
        ui,
        &settings.theme_id,
        &settings.theme_variant,
        &settings.accent_color,
        &settings.dynamic_color_style,
        settings.last_static_accent(&settings.theme_id),
        system,
    );
}

/// Resolves a possibly stale pick, seats every `Settings` row and applies the palette. `style_id`
/// seeds only the Color Style chips; whether Material You is active is
/// `system.material_you.is_some()`, which the coordinator publishes once it has extracted.
///
/// `last_static_accent` is what a Material You accent paints while Material You isn't active,
/// rather than the theme's default.
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

    // The swatches take the variant System resolves to, so they draw in the shade painted.
    let accent_variant_for_swatches = theme.shade_for(resolved_variant, &system.theme).id;
    let (brushes, labels) = accent_swatches(theme, accent_variant_for_swatches);
    let g_swatches = ui.global::<Settings>();
    g_swatches.set_accent_colors(ModelRc::from(Rc::new(VecModel::from(brushes))));
    g_swatches.set_accent_names(ModelRc::from(Rc::new(VecModel::from(labels))));

    // A Material You accent with nothing to follow yet paints the last static pick, an accent
    // the user did choose.
    let my_active = material_you_active(theme.id, system);
    let effective_id = effective_accent_id(theme, accent_id, my_active, last_static_accent);
    let style_idx = SchemeStyle::all().iter().position(|s| s.as_id() == style_id).unwrap_or(0);
    // The switch keeps reading on while nothing is playing, so it seats the persisted pick. Only
    // with no Color Style, where there is no artwork to follow, does the painted fallback show.
    let can_follow = theme.id == themes::material3::MATERIAL3.id
        && SchemeStyle::from_id(style_id) != SchemeStyle::None;
    let seated_id = if can_follow { accent_id } else { effective_id };

    let g = ui.global::<Settings>();
    g.set_theme_idx(apply_and_seed_to_i32(theme_idx));
    g.set_variant_idx(apply_and_seed_to_i32(variant_idx));
    seat_accent(&g, theme, seated_id);
    g.set_color_style_idx(apply_and_seed_to_i32(style_idx));
    g.set_kde_system_active(theme_picker::kde_system_active(theme.id, variant_id, system));
    g.set_material_you_active(my_active);

    super::apply_palette(ui, theme.id, resolved_variant, effective_id, system);
}
