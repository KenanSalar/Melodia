//! The Color Style picker. A pick crossing None writes an accent beside the style, and both
//! writes and the coordinator's kick share one blocking task, so the coordinator wakes to both.

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
        let outgoing = last_style.replace(style);

        // Read here, the blocking task below having no UI access.
        let material3 = registry_get(g.get_theme_idx()).filter(|t| t.id == MATERIAL3.id);
        let variant_id: Option<String> = material3.map(|t| {
            variant_id_at(t, usize_from(g.get_variant_idx()))
                .unwrap_or(t.default_variant)
                .to_owned()
        });

        // Off the shadow, which an accent click whose write hasn't committed has already moved.
        let new_accent: Option<String> = material3.and_then(|theme| {
            let change = accent_change(outgoing, style, &persisted_accent.lock());
            match change {
                AccentChange::Keep => None,
                AccentChange::Follow => Some(MATERIAL_YOU_ACCENT_ID.to_owned()),
                AccentChange::Restore => Some(
                    read_last_static_accent(&s, theme.id)
                        .unwrap_or_else(|| theme.default_accent.to_owned()),
                ),
            }
        });

        // The shadow before the write, so a sibling `wire_variant_changed` landing first reads the
        // new accent. Seated here rather than left to the coordinator's repaint, which a style
        // change with nothing playing never sends.
        if let (Some(theme), Some(accent)) = (material3, new_accent.as_deref()) {
            accent.clone_into(&mut persisted_accent.lock());
            seat_accent(&g, theme, accent);
        }

        // `read_settings` takes no writer's lock, so the kick has to follow both commits.
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

            // A partial write would wake the coordinator to a style and an accent that disagree.
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

/// What a Color Style pick does to the Material 3 accent.
#[derive(Debug, PartialEq, Eq)]
enum AccentChange {
    Keep,
    /// Follow the artwork.
    Follow,
    /// Take back the last accent picked by hand.
    Restore,
}

/// Following the artwork comes on only on the way out of None: a new style over an accent picked
/// by hand is a different palette, not a request to follow the artwork again. None has no artwork
/// to follow, so it hands a following accent back.
fn accent_change(outgoing: SchemeStyle, incoming: SchemeStyle, accent_id: &str) -> AccentChange {
    let following = accent_id == MATERIAL_YOU_ACCENT_ID;
    match incoming {
        SchemeStyle::None if following => AccentChange::Restore,
        SchemeStyle::None => AccentChange::Keep,
        _ if outgoing == SchemeStyle::None && !following => AccentChange::Follow,
        _ => AccentChange::Keep,
    }
}

#[cfg(test)]
#[path = "tests/material_you_sync_tests.rs"]
mod tests;
