//! Theme + variant chip wiring + the per-theme memory lookup
//! ([`lookup_remembered`]). Re-uses [`super::apply_and_seed`] for the
//! orchestration after a theme switch.

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::RwLock;
use slint::{ComponentHandle, ModelRc, VecModel};

use melodia_app::library;
use melodia_app::state::{AppState, Signal};
use melodia_core::themes::{self, MATERIAL_YOU_ACCENT_ID, SystemColorState, ThemeDef};
use melodia_ui::{AppWindow, Settings};

use super::accent_picker::{
    accent_id_from_grid_idx, accent_swatches_with_my, effective_accent_id, material_you_active,
};
use super::{
    PersistedAccent, apply_and_seed, persist_and_kick, read_last_static_accent, registry_get,
    usize_from,
};

/// True when KDE Breeze + System is active *and* a parsed `kdeglobals` palette is in
/// hand — the palette coming from Plasma's live scheme rather than the static tables.
/// The Settings page reads it to hide the Accent Color row, Plasma having already
/// picked one.
pub(super) fn kde_system_active(
    theme_id: &str,
    variant_id: &str,
    system: &SystemColorState,
) -> bool {
    if theme_id != "kde-breeze" || variant_id != themes::SYSTEM_VARIANT_ID {
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

/// Index of `variant_id` in the chip list, accounting for the synthetic
/// "System" chip appended at the end when the theme opts in.
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

        // Per-theme memory: each theme remembers its last (variant, accent) pair across
        // switches. Read synchronously on the UI thread — `settings.json` is tiny and a
        // chip click coarse-grained, so the read is invisible next to the click. Falls
        // back to the theme's canonical defaults on a miss or a read failure.
        let (variant_id, accent_id) = lookup_remembered(&s, theme);
        let style_id = library::settings::get_settings(&s)
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
        // Before the async write, so a sibling firing ahead of the disk — a variant
        // click straight after a theme one — reads the new accent, not the old.
        accent_id.clone_into(&mut persisted_accent.lock());
        // Sequenced inside one `spawn_blocking`, so the coordinator reads the new theme
        // on wake rather than the previous one — switching to or from material3 flips
        // whether dynamic colour applies at all.
        persist_and_kick(&s, theme.id, &variant_id, &accent_id, &kick);
    });
}

/// Resolve `(variant_id, accent_id)` from `settings.theme_preferences[theme.id]`,
/// falling back to the theme's canonical defaults when the entry is missing, the read
/// fails, or the stored ids no longer exist in the registry.
fn lookup_remembered(state: &AppState, theme: &ThemeDef) -> (String, String) {
    let defaults = || (theme.default_variant.to_owned(), theme.default_accent.to_owned());
    let Ok(settings) = library::settings::get_settings(state) else {
        return defaults();
    };
    let Some(pref) = settings.theme_preferences.get(theme.id) else {
        return defaults();
    };
    // Against the current registry: palette ids may have changed across upgrades. The
    // synthetic "system" id passes through even though it isn't in `theme.variants`.
    let variant_known = theme.variant(&pref.variant).is_some()
        || (pref.variant == themes::SYSTEM_VARIANT_ID && theme.supports_system_mode);
    let variant = if variant_known {
        pref.variant.clone()
    } else {
        theme.default_variant.to_owned()
    };
    // Material 3 alone accepts the synthetic `MATERIAL_YOU_ACCENT_ID`: it is in no
    // `theme.accents`, having no static swatch, but is a legitimate persisted value the
    // dynamic-colour pipeline resolves at paint time. Without this, switching themes
    // through material3 forgets the user's pick on every round trip.
    let accent_known = theme.accent(&pref.accent).is_some()
        || (theme.id == "material3" && pref.accent == MATERIAL_YOU_ACCENT_ID);
    let accent = if accent_known {
        pref.accent.clone()
    } else {
        theme.default_accent.to_owned()
    };
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
        let theme_idx = g.get_theme_idx();
        let accent_idx = g.get_accent_idx();
        let Some(theme) = registry_get(theme_idx) else {
            return;
        };

        let i = usize_from(idx);
        // The synthetic "System" chip sits at `theme.variants.len()` when the theme
        // opts in; anything past that bails. Slint clamps the index already, but the
        // boundary shouldn't trust it.
        let variant_id: &str = if theme.supports_system_mode && i == theme.variants.len() {
            themes::SYSTEM_VARIANT_ID
        } else if let Some(v) = theme.variants.get(i) {
            v.id
        } else {
            return;
        };

        let snapshot = os_state.read().clone();
        let my_active = material_you_active(theme.id, &snapshot);

        // The shadow cell rather than `settings.json`: a sibling `wire_accent_changed`
        // can have updated the cell without committing its write yet, and a disk read
        // here would then observe the old value and persist it, silently undoing the
        // user's pick. Every `wire_*` updates the cell before its async write.
        let persisted_accent_str = persisted_accent.lock().clone();
        let last_static_owned = read_last_static_accent(&s, theme.id);
        let visible_accent_id = if my_active {
            // The grid index is authoritative while Material You is active: its own dot
            // and the statics are all clickable.
            accent_id_from_grid_idx(theme, usize_from(accent_idx), true)
                .unwrap_or(theme.default_accent)
        } else {
            // Material You isn't a clickable dot here, so the visible accent is whichever
            // static the grid highlights — but `persisted_accent` is still what gets
            // written, so a sticky pick stays sticky.
            effective_accent_id(theme, &persisted_accent_str, false, last_static_owned.as_deref())
        };

        // Against the resolved *real* variant, which matters on a switch to "System":
        // the swatches render dark or light off the OS signal.
        let accent_variant = if variant_id == themes::SYSTEM_VARIANT_ID {
            theme.resolve_system_variant(&snapshot.theme).id
        } else {
            variant_id
        };
        let (brushes, labels, _) = accent_swatches_with_my(theme, accent_variant, &snapshot);
        let g_swatches = ui.global::<Settings>();
        g_swatches.set_accent_colors(ModelRc::from(Rc::new(VecModel::from(brushes))));
        g_swatches.set_accent_names(ModelRc::from(Rc::new(VecModel::from(labels))));
        g.set_variant_idx(idx);
        g.set_kde_system_active(kde_system_active(theme.id, variant_id, &snapshot));
        super::apply_palette(&ui, theme.id, variant_id, visible_accent_id, &snapshot);
        // The *original* accent: a variant flip never demotes a sticky Material You
        // pick to its static fallback, and the shadow cell already holds this value.
        // Write and kick are sequenced in one `spawn_blocking`, so the coordinator
        // reads the new variant on wake — a flip can change `is_dark` and oblige a
        // regenerate.
        persist_and_kick(&s, theme.id, variant_id, &persisted_accent_str, &kick);
    });
}
