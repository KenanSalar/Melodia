//! The theme and variant chips, and the variant and accent each theme remembers across a switch
//! ([`lookup_remembered`]).

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::RwLock;
use slint::{ComponentHandle, ModelRc, VecModel};

use melodia_app::library;
use melodia_app::state::{AppState, Signal};
use melodia_core::themes::{self, MATERIAL_YOU_ACCENT_ID, SystemColorState, ThemeDef};
use melodia_ui::{AppWindow, Settings};

use super::accent_picker::{accent_swatches, effective_accent_id, material_you_active};
use super::{
    PersistedAccent, apply_and_seed, persist_and_kick, read_last_static_accent, registry_get,
    usize_from, variant_id_at,
};

/// Whether KDE Breeze's System variant is painting Plasma's live scheme, which hides the Accent
/// Color row, Plasma having picked one.
pub(super) fn kde_system_active(
    theme_id: &str,
    variant_id: &str,
    system: &SystemColorState,
) -> bool {
    if theme_id != themes::kde::KDE.id || variant_id != themes::SYSTEM_VARIANT_ID {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        system.kde_palette.is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = system;
        false
    }
}

/// `variant_id`'s chip, System's being the one after the theme's own where it offers one.
pub(super) fn variant_index_with_system(theme: &ThemeDef, variant_id: &str) -> usize {
    if variant_id == themes::SYSTEM_VARIANT_ID && theme.supports_system_mode {
        return theme.variants.len();
    }
    themes::variant_index(theme, variant_id)
}

pub(super) fn wire_theme_changed(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    kick: Signal,
    persisted_accent: PersistedAccent,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_theme_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(theme) = registry_get(idx) else {
            return;
        };

        // A synchronous read: the file is tiny and a chip click coarse-grained.
        let (variant_id, accent_id) = lookup_remembered(&s, theme);
        let style_id = library::settings::get_settings(&s.paths)
            .map_or_else(|_| "none".to_owned(), |c| c.dynamic_color_style);
        let last_static = read_last_static_accent(&s, theme.id);
        let snapshot = os_state.read().clone();
        apply_and_seed(
            &ui,
            theme.id,
            &variant_id,
            &accent_id,
            &style_id,
            last_static.as_deref(),
            &snapshot,
        );
        accent_id.clone_into(&mut persisted_accent.lock());
        // A switch to or from Material 3 decides whether dynamic colour applies at all.
        persist_and_kick(&s, theme.id, &variant_id, &accent_id, &kick);
    });
}

/// The theme's remembered `(variant_id, accent_id)`, or its defaults where nothing is remembered,
/// the read fails, or a stored id is no longer in the registry.
fn lookup_remembered(state: &AppState, theme: &ThemeDef) -> (String, String) {
    let defaults = || (theme.default_variant.to_owned(), theme.default_accent.to_owned());
    let Ok(settings) = library::settings::get_settings(&state.paths) else {
        return defaults();
    };
    let Some(pref) = settings.theme_preferences.get(theme.id) else {
        return defaults();
    };
    // Ids can change across upgrades. System's is in no `theme.variants` and passes anyway.
    let variant_known = theme.variant(&pref.variant).is_some()
        || (pref.variant == themes::SYSTEM_VARIANT_ID && theme.supports_system_mode);
    let variant =
        if variant_known { pref.variant.clone() } else { theme.default_variant.to_owned() };
    // Material 3 alone takes `MATERIAL_YOU_ACCENT_ID`, which has no swatch to be in
    // `theme.accents`; refused, a round trip through another theme would forget it.
    let accent_known = theme.accent(&pref.accent).is_some()
        || (theme.id == themes::material3::MATERIAL3.id && pref.accent == MATERIAL_YOU_ACCENT_ID);
    let accent = if accent_known { pref.accent.clone() } else { theme.default_accent.to_owned() };
    (variant, accent)
}

pub(super) fn wire_variant_changed(
    ui: &AppWindow,
    state: &AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    kick: Signal,
    persisted_accent: PersistedAccent,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_variant_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let Some(theme) = registry_get(g.get_theme_idx()) else {
            return;
        };
        // Slint clamps the index already, but the boundary shouldn't trust it.
        let Some(variant_id) = variant_id_at(theme, usize_from(idx)) else {
            return;
        };

        let snapshot = os_state.read().clone();
        let my_active = material_you_active(theme.id, &snapshot);

        // Off the shadow: a file read could find the accent a sibling's uncommitted write is
        // replacing, and persist it back over the user's pick.
        let persisted_accent_str = persisted_accent.lock().clone();
        let last_static_owned = read_last_static_accent(&s, theme.id);
        let visible_accent_id = effective_accent_id(
            theme,
            &persisted_accent_str,
            my_active,
            last_static_owned.as_deref(),
        );

        // The variant System resolves to, so the swatches draw in the shade painted.
        let accent_variant = theme.shade_for(variant_id, &snapshot.theme).id;
        let (brushes, labels) = accent_swatches(theme, accent_variant);
        let g_swatches = ui.global::<Settings>();
        g_swatches.set_accent_colors(ModelRc::from(Rc::new(VecModel::from(brushes))));
        g_swatches.set_accent_names(ModelRc::from(Rc::new(VecModel::from(labels))));
        g.set_variant_idx(idx);
        g.set_kde_system_active(kde_system_active(theme.id, variant_id, &snapshot));
        super::apply_palette(&ui, theme.id, variant_id, visible_accent_id, &snapshot);
        // The persisted accent, so a flip never demotes a Material You pick to its fallback. The
        // coordinator owes a regenerate where the flip moves `is_dark`.
        persist_and_kick(&s, theme.id, variant_id, &persisted_accent_str, &kick);
    });
}
