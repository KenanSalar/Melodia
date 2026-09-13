//! The Window Border rows: whether a frameless window draws the 1 px outline an OS draws round its
//! own frames, and in what colour.
//!
//! **System is the OS's own colour.** On Windows that is the accent while the user has it on window
//! borders. Everywhere else, and on Windows with it off, it is a neutral mixed from the palette's
//! window surface and its text, which is how KDE's Breeze outline and the macOS and GNOME hairlines
//! sit against a window. Mixed rather than fixed, so it keeps its weight on a light palette and a
//! dark one alike.
//!
//! The other swatches are the theme's accents, and a pick is stored as the accent's id, so it
//! follows a variant's shading the way the accent row does. An id the theme doesn't have paints as
//! System.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use melodia_app::library;
use melodia_app::services::settings::{WINDOW_BORDER_SYSTEM_COLOR, WindowBorder, WindowFlags};
use melodia_app::state::AppState;
use melodia_core::themes::{self, SYSTEM_VARIANT_ID, SystemColorState, ThemeDef};
use melodia_ui::{AppWindow, Settings, Theme, WindowChrome};

use super::theme_apply::{accent_brushes, brush, brush_to_rgb};

/// The neutral border's share of the text colour, out of [`NEUTRAL_PARTS`] with the rest the
/// window surface: enough to read as an edge against the desktop, little enough not to read as a
/// rule across the window.
const NEUTRAL_INK_PARTS: u32 = 1;
const NEUTRAL_PARTS: u32 = 5;

thread_local! {
    /// The persisted colour id. A palette change re-resolves the pick from here rather than from
    /// `settings.json`, which a pick may still be writing.
    static COLOR_ID: RefCell<String> = RefCell::new(WINDOW_BORDER_SYSTEM_COLOR.to_owned());
}

/// Seed both rows from the persisted flags. Has to run ahead of the first palette apply, which
/// resolves the colour seeded here.
pub(super) fn seed(ui: &AppWindow, window: &WindowFlags) {
    ui.global::<Settings>().set_window_border_shown(window.window_border == WindowBorder::Shown);
    COLOR_ID.with_borrow_mut(|id| id.clone_from(&window.window_border_color));
}

/// Re-seed the swatch grid and repaint the outline against the palette `theme_apply` just wrote.
pub(super) fn republish_for_palette(
    ui: &AppWindow,
    theme_id: &str,
    variant_id: &str,
    system: &SystemColorState,
) {
    let theme = themes::get(theme_id);
    let shade = if variant_id == SYSTEM_VARIANT_ID && theme.supports_system_mode {
        theme.resolve_system_variant(&system.theme).id
    } else {
        theme.resolved_variant(variant_id).id
    };
    let neutral = palette_neutral(ui);
    let os_accent = os_accent();

    let mut colors = accent_brushes(theme, shade);
    colors.insert(0, brush(system_border(os_accent, neutral).0));
    let names: Vec<SharedString> = std::iter::once(SharedString::new())
        .chain(theme.accents.iter().map(|a| SharedString::from(a.name)))
        .collect();

    let (slot, picked) =
        COLOR_ID.with_borrow(|id| (swatch_index(theme, id), theme.accent_hex(id, shade)));
    let g = ui.global::<Settings>();
    g.set_window_border_colors(ModelRc::from(Rc::new(VecModel::from(colors))));
    g.set_window_border_names(ModelRc::from(Rc::new(VecModel::from(names))));
    g.set_window_border_color_idx(super::apply_and_seed_to_i32(slot));
    paint(ui, border_colors(picked, os_accent, neutral));
}

/// Repaint a System outline against the OS's current answer.
///
/// Windows lets the accent go on or off window borders with Melodia running and tells a client
/// nothing, so a focus gain, the first thing after a trip to the Settings app, asks again.
#[cfg(target_os = "windows")]
pub fn refresh_system_color(ui: &AppWindow) {
    if ui.global::<Settings>().get_window_border_color_idx() != 0 {
        return;
    }
    paint(ui, border_colors(None, os_accent(), palette_neutral(ui)));
}

pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_window_border_shown_changed(move |shown| {
        let border = if shown { WindowBorder::Shown } else { WindowBorder::Hidden };
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::window::set_window_border(&s_clone, border) {
                log::warn!("persist window_border: {e}");
            }
        });
    });

    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_window_border_color_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let Some(theme) = super::registry_get(g.get_theme_idx()) else { return };
        let slot = super::usize_from(idx);
        let Some(color_id) = color_id_at(theme, slot) else { return };

        g.set_window_border_color_idx(idx);
        // The swatch is the pick already shaded for the live variant, which saves resolving it.
        let picked = (slot > 0)
            .then(|| g.get_window_border_colors().row_data(slot))
            .flatten()
            .map(|swatch| brush_to_rgb(&swatch));
        paint(&ui, border_colors(picked, os_accent(), palette_neutral(&ui)));

        // Ahead of the write, so a palette change landing before it commits re-resolves this pick.
        COLOR_ID.with_borrow_mut(|id| color_id.clone_into(id));
        let s_clone = s.clone();
        s.runtime.spawn_blocking(move || {
            if let Err(e) = library::window::set_window_border_color(&s_clone, color_id.to_owned())
            {
                log::warn!("persist window_border_color: {e}");
            }
        });
    });
}

fn paint(ui: &AppWindow, (focused, unfocused): (u32, u32)) {
    let chrome = ui.global::<WindowChrome>();
    chrome.set_border_color(brush(focused));
    chrome.set_border_color_unfocused(brush(unfocused));
}

/// The neutral border for the palette `Theme` holds now.
fn palette_neutral(ui: &AppWindow) -> u32 {
    let theme = ui.global::<Theme>();
    neutral_border(brush_to_rgb(&theme.get_mantle()), brush_to_rgb(&theme.get_text()))
}

#[cfg(target_os = "windows")]
fn os_accent() -> Option<u32> {
    melodia_platform::services::platform::window_border::accent_border_rgb()
}

/// Only Windows puts its accent on window borders.
#[cfg(not(target_os = "windows"))]
fn os_accent() -> Option<u32> {
    None
}

/// The outline's `(focused, unfocused)` colours: a picked accent in both, otherwise System's.
fn border_colors(picked: Option<u32>, os_accent: Option<u32>, neutral: u32) -> (u32, u32) {
    picked.map_or_else(|| system_border(os_accent, neutral), |accent| (accent, accent))
}

/// System's `(focused, unfocused)` colours. Windows draws its accent on the active window alone and
/// takes an inactive one back to neutral.
fn system_border(os_accent: Option<u32>, neutral: u32) -> (u32, u32) {
    (os_accent.unwrap_or(neutral), neutral)
}

/// `surface` moved [`NEUTRAL_INK_PARTS`] of [`NEUTRAL_PARTS`] toward `ink`, per channel.
fn neutral_border(surface: u32, ink: u32) -> u32 {
    let channel = |shift: u32| {
        let s = (surface >> shift) & 0xFF;
        let i = (ink >> shift) & 0xFF;
        ((s * (NEUTRAL_PARTS - NEUTRAL_INK_PARTS) + i * NEUTRAL_INK_PARTS) / NEUTRAL_PARTS) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// The grid slot a persisted id sits in: System first, then the theme's accents in order. An id
/// the theme doesn't have is System, the colour it paints as.
fn swatch_index(theme: &ThemeDef, color_id: &str) -> usize {
    theme.accents.iter().position(|a| a.id == color_id).map_or(0, |at| at + 1)
}

/// The id a clicked slot stands for, `None` past the grid's end.
fn color_id_at(theme: &ThemeDef, slot: usize) -> Option<&'static str> {
    match slot.checked_sub(1) {
        None => Some(WINDOW_BORDER_SYSTEM_COLOR),
        Some(accent) => theme.accents.get(accent).map(|a| a.id),
    }
}

#[cfg(test)]
#[path = "tests/window_border_tests.rs"]
mod tests;
