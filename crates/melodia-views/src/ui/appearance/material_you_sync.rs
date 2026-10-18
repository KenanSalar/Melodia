//! `SchemeStyle` picker (`wire_color_style_changed`). Switching the colour
//! style can simultaneously persist a new style + a flipped accent when
//! crossing the None/non-None boundary; both writes plus the Material You
//! coordinator kick are sequenced inside one `spawn_blocking` so the
//! coordinator's `settings.json` read on wake-up sees both values
//! committed.

use std::cell::Cell;

use slint::ComponentHandle;

use melodia_app::library;
use melodia_app::state::{AppState, Signal};
use melodia_artwork::media::image::material_you::SchemeStyle;
use melodia_core::error::describe;
use melodia_core::themes::MATERIAL_YOU_ACCENT_ID;
use melodia_core::themes::material3::MATERIAL3;
use melodia_ui::{AppWindow, Settings};

use super::accent_picker::seat_accent;
use super::{PersistedAccent, read_last_static_accent, registry_get, usize_from, variant_id_at};

pub(super) fn wire_color_style_changed(
    ui: &AppWindow,
    state: &AppState,
    kick: Signal,
    persisted_accent: PersistedAccent,
) {
    let weak = ui.as_weak();
    let s = state.clone();
    // The outgoing style, which `color-style-idx` no longer holds by the time the chips emit.
    // `apply_settings` seats the persisted one before this is wired.
    let last_style = Cell::new(style_at(ui.global::<Settings>().get_color_style_idx()));
    ui.global::<Settings>().on_color_style_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let style = style_at(idx);
        let was_none = last_style.replace(style) == SchemeStyle::None;

        // Snapshot the theme + variant on the UI thread so the spawn_blocking task below doesn't
        // need any UI access.
        let material3 = registry_get(g.get_theme_idx()).filter(|t| t.id == MATERIAL3.id);
        let variant_id: Option<String> = material3.map(|t| {
            variant_id_at(t, usize_from(g.get_variant_idx()))
                .unwrap_or(t.default_variant)
                .to_owned()
        });

        // Off the shadow, which an accent click whose write hasn't committed has already moved.
        // Following the artwork comes on only on the way out of None: a new style over an accent
        // picked by hand is a different palette, not a request to follow the artwork again.
        let new_accent: Option<String> = material3.and_then(|theme| {
            let following = *persisted_accent.lock() == MATERIAL_YOU_ACCENT_ID;
            match style {
                SchemeStyle::None if following => Some(
                    read_last_static_accent(&s, theme.id)
                        .unwrap_or_else(|| theme.default_accent.to_owned()),
                ),
                SchemeStyle::None => None,
                _ if was_none && !following => Some(MATERIAL_YOU_ACCENT_ID.to_owned()),
                _ => None,
            }
        });

        // The shadow before the write, so a sibling `wire_variant_changed` landing first reads the
        // new accent. Seated here rather than left to the coordinator's repaint, which a style
        // change with nothing playing never sends.
        if let (Some(theme), Some(accent)) = (material3, new_accent.as_deref()) {
            accent.clone_into(&mut persisted_accent.lock());
            seat_accent(&g, theme, accent);
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
        s.runtime.spawn_blocking(move || {
            let mut all_persists_ok = true;
            if let Err(e) = library::settings::set_dynamic_color_style(&s_clone.paths, style_owned)
            {
                log::warn!("persist material3 colour style: {}", describe(&e));
                all_persists_ok = false;
            }

            if let (Some(theme), Some(variant_id), Some(accent)) =
                (material3, variant_id, new_accent)
                && let Err(e) = library::settings::set_appearance(
                    &s_clone.paths,
                    theme.id.to_owned(),
                    variant_id,
                    accent,
                )
            {
                log::warn!("persist material3 accent: {}", describe(&e));
                all_persists_ok = false;
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

/// The style a chip index stands for, `SchemeStyle::None` past the end.
fn style_at(idx: i32) -> SchemeStyle {
    SchemeStyle::all().get(usize_from(idx)).copied().unwrap_or(SchemeStyle::None)
}
