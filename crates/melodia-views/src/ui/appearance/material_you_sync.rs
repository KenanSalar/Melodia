//! `SchemeStyle` picker (`wire_color_style_changed`). Switching the colour
//! style can simultaneously persist a new style + a flipped accent when
//! crossing the None/non-None boundary; both writes plus the Material You
//! coordinator kick are sequenced inside one `spawn_blocking` so the
//! coordinator's `settings.json` read on wake-up sees both values
//! committed.

use std::sync::Arc;

use parking_lot::RwLock;
use slint::ComponentHandle;

use crate::{AppWindow, Settings};
use melodia_app::library;
use melodia_app::state::{AppState, Signal};
use melodia_artwork::media::image::material_you::SchemeStyle;
use melodia_core::themes::{self, MATERIAL_YOU_ACCENT_ID, SystemColorState};

use super::{PersistedAccent, registry_get, usize_from};

pub(super) fn wire_color_style_changed(
    ui: &AppWindow,
    state: &AppState,
    _os_state: Arc<RwLock<SystemColorState>>,
    kick: Signal,
    persisted_accent: PersistedAccent,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_color_style_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let style = SchemeStyle::all().get(usize_from(idx)).copied().unwrap_or(SchemeStyle::None);

        // Snapshot the variant + theme on the UI thread so the
        // spawn_blocking task below doesn't need any UI access. Owned
        // strings keep the closure 'static.
        let theme_idx = g.get_theme_idx();
        let variant_idx = g.get_variant_idx();
        let on_material3 = registry_get(theme_idx).is_some_and(|t| t.id == "material3");
        let theme_id: Option<&'static str> = registry_get(theme_idx).map(|t| t.id);
        let variant_id_owned: Option<String> = registry_get(theme_idx).map(|t| {
            let i = usize_from(variant_idx);
            if t.supports_system_mode && i == t.variants.len() {
                themes::SYSTEM_VARIANT_ID.to_owned()
            } else if let Some(v) = t.variants.get(i) {
                v.id.to_owned()
            } else {
                t.default_variant.to_owned()
            }
        });
        let theme_default_accent: Option<&'static str> =
            registry_get(theme_idx).map(|t| t.default_accent);

        // Resolve the new accent on the UI thread using the synchronous
        // accent shadow — `current_accent` from the cell is always at
        // least as fresh as the disk, so a recently-clicked accent that
        // hasn't yet committed still drives the right transition. The
        // `last_static` fallback (only used on !None → None when the
        // current accent is "material_you") is read inside the
        // spawn_blocking below since it's a nested per-theme field;
        // acceptable because that branch only fires when the user
        // explicitly disables Color Style, which they typically don't
        // mash against an accent click.
        let new_accent: Option<String> = if on_material3 {
            let current_accent = persisted_accent.lock().clone();
            match style {
                SchemeStyle::None if current_accent == MATERIAL_YOU_ACCENT_ID => {
                    Some(String::new())
                }
                SchemeStyle::None => None,
                _ if current_accent == MATERIAL_YOU_ACCENT_ID => None,
                _ => Some(MATERIAL_YOU_ACCENT_ID.to_owned()),
            }
        } else {
            None
        };

        // Update the synchronous accent shadow before the async writes
        // so a sibling `wire_variant_changed` that fires before the
        // disk commit reads the new accent. Empty string is the
        // sentinel for "resolve to last_static inside spawn_blocking";
        // we update the cell to that resolved value once the
        // spawn_blocking task knows it.
        if let Some(ref accent) = new_accent
            && !accent.is_empty()
        {
            persisted_accent.lock().clone_from(accent);
        }

        // Sequence both writes + the kick inside a single spawn_blocking
        // so the coordinator's settings.json read on wake-up sees both
        // the new style and the new accent. Firing the kick before the
        // disk writes commit was racy — `mutate_settings` serializes
        // writers via a mutex, but `read_settings` doesn't take that
        // lock, so a kick raced ahead of the write would let the
        // coordinator regenerate against a stale style.
        let s_clone = s.clone();
        let kick_clone = kick.clone();
        let style_owned = style.as_id().to_owned();
        let persisted_accent_for_blocking = persisted_accent.clone();
        s.runtime.spawn_blocking(move || {
            let mut all_persists_ok = true;
            if let Err(e) = library::settings::set_dynamic_color_style(&s_clone, style_owned) {
                log::warn!("persist material3 colour style: {e}");
                all_persists_ok = false;
            }

            if on_material3
                && let (Some(theme_id), Some(variant_id), Some(default_accent), Some(new_accent)) =
                    (theme_id, variant_id_owned, theme_default_accent, new_accent)
            {
                // Empty string is the sentinel that means "resolve to
                // the per-theme `last_static_accent` from disk now".
                // The disk is authoritative for last_static because
                // every wire_accent_changed write hits theme_preferences
                // through `set_appearance` and `mutate_settings`
                // serialises those — so by the time we read here,
                // any in-flight accent write has been ordered against
                // ours via the same `MUTATE_LOCK`.
                let resolved_accent = if new_accent.is_empty() {
                    let last_static = library::settings::get_settings(&s_clone)
                        .ok()
                        .and_then(|c| c.theme_preferences.get(theme_id).cloned())
                        .and_then(|p| p.last_static_accent)
                        .unwrap_or_else(|| default_accent.to_owned());
                    persisted_accent_for_blocking.lock().clone_from(&last_static);
                    last_static
                } else {
                    new_accent
                };
                if let Err(e) = library::settings::set_appearance(
                    &s_clone,
                    theme_id.to_owned(),
                    variant_id,
                    resolved_accent,
                ) {
                    log::warn!("persist material3 accent: {e}");
                    all_persists_ok = false;
                }
            }

            // Kick AFTER both disk writes commit so the coordinator's
            // settings.json read observes the new style + accent. Suppress
            // the kick on a partial-write failure — otherwise the
            // coordinator wakes and reads inconsistent (style ≠ accent)
            // settings, regenerates against a frozen-in-time pair, and
            // goes back to sleep until the next user action.
            if all_persists_ok {
                kick_clone.bump();
            } else {
                log::warn!(
                    "material3 colour-style persist had write failures; \
                     suppressing Material You kick"
                );
            }
        });
    });
}
