//! The Window Border rows: whether a frameless window draws the 1 px outline an OS draws round its
//! own frames, and in what colour.
//!
//! **System is the OS's own colour, and a switch rather than a swatch**, since what it paints is
//! the OS's answer and no swatch could show it. On Windows that is the accent while the user has it
//! on window borders. Everywhere else, and on Windows with it off, it is a neutral mixed from the
//! palette's window surface and its text, which is how KDE's Breeze outline and the macOS and GNOME
//! hairlines sit against a window. Mixed rather than fixed, so it keeps its weight on a light
//! palette and a dark one alike. Under Material You the palette is the artwork's, so the neutral
//! follows the track unless [`BorderTint::Theme`] mixes it from the theme's own instead.
//!
//! The swatches are the accent row's, and a pick is stored as the accent's id, so it follows a
//! variant's shading the way the accent row does. An id the theme doesn't have paints as System.

use std::cell::{Cell, RefCell};

use slint::{ComponentHandle, Model};

use melodia_app::library;
use melodia_app::services::settings::{
    BorderTint, WINDOW_BORDER_SYSTEM_COLOR, WindowBorder, WindowFlags,
};
use melodia_app::state::AppState;
use melodia_core::themes::{self, SystemColorState, ThemeDef};
use melodia_ui::{AppWindow, Settings, Theme, WindowChrome};

use super::accent_picker::{accent_at, accent_slot, material_you_active, slot_index};
use super::rgb::{brush, brush_to_rgb};
use super::theme_paint;

/// The neutral border's share of the text colour, out of [`NEUTRAL_PARTS`] with the rest the
/// window surface: enough to read as an edge against the desktop, little enough not to read as a
/// rule across the window.
const NEUTRAL_INK_PARTS: u32 = 1;
const NEUTRAL_PARTS: u32 = 5;

thread_local! {
    /// The persisted colour id. A palette change re-resolves the pick from here rather than from
    /// `settings.json`, which a pick may still be writing.
    static COLOR_ID: RefCell<String> = RefCell::new(WINDOW_BORDER_SYSTEM_COLOR.to_owned());
    /// The persisted tint, mirrored ahead of its write as the colour id is.
    static TINT: Cell<BorderTint> = const { Cell::new(BorderTint::Artwork) };
    /// The neutral off the theme's own palette while Material You paints the artwork's, `None`
    /// while the theme's is the one painted. Re-solved on every palette change, so the tint switch
    /// can repaint without one.
    static THEME_NEUTRAL: Cell<Option<u32>> = const { Cell::new(None) };
}

/// Seed the rows from the persisted flags. Has to run ahead of the first palette apply, which
/// resolves the colour seeded here.
pub(super) fn seed(ui: &AppWindow, window: &WindowFlags) {
    let g = ui.global::<Settings>();
    g.set_window_border_shown(window.window_border == WindowBorder::Shown);
    g.set_window_border_material_you(window.window_border_tint == BorderTint::Artwork);
    COLOR_ID.with_borrow_mut(|id| id.clone_from(&window.window_border_color));
    TINT.set(window.window_border_tint);
}

/// Re-seat the pick and repaint the outline against the palette `theme_paint` just painted.
pub(super) fn republish_for_palette(
    ui: &AppWindow,
    theme_id: &str,
    variant_id: &str,
    system: &SystemColorState,
) {
    let theme = themes::get(theme_id);
    let shade = theme.shade_for(variant_id, &system.theme);
    THEME_NEUTRAL.set(
        material_you_active(theme_id, system)
            .then(|| neutral_border(shade.palette.mantle, shade.palette.text)),
    );
    let (slot, picked) =
        COLOR_ID.with_borrow(|id| (accent_slot(theme, id), theme.accent_hex(id, shade.id)));
    seat(&ui.global::<Settings>(), slot);
    paint(ui, border_colors(picked, os_accent(), system_neutral(ui)));
}

/// Repaint a System outline against the OS's current answer.
///
/// Windows lets the accent go on or off window borders with Melodia running and tells a client
/// nothing, so a focus gain, the first thing after a trip to the Settings app, asks again.
#[cfg(target_os = "windows")]
pub fn refresh_system_color(ui: &AppWindow) {
    repaint_system(ui);
}

pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Settings>().on_window_border_shown_changed(move |shown| {
        let border = if shown { WindowBorder::Shown } else { WindowBorder::Hidden };
        s.persist_blocking("persist window_border", move |paths| {
            library::window::set_window_border(paths, border)
        });
    });

    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_window_border_system_changed(move |system| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let Some(theme) = super::registry_get(g.get_theme_idx()) else { return };
        // Off takes the accent row's pick, the colour an outline most often matches, or the
        // theme's default while that row follows the artwork.
        let color_id = if system {
            WINDOW_BORDER_SYSTEM_COLOR
        } else {
            accent_at(theme, super::usize_from(g.get_accent_idx())).unwrap_or(theme.default_accent)
        };
        pick(&ui, &s, theme, color_id);
    });

    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_window_border_color_changed(move |idx| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Settings>();
        let Some(theme) = super::registry_get(g.get_theme_idx()) else { return };
        let Some(color_id) = accent_at(theme, super::usize_from(idx)) else { return };
        pick(&ui, &s, theme, color_id);
    });

    let weak = ui.as_weak();
    let s = state.clone();
    ui.global::<Settings>().on_window_border_material_you_changed(move |tinted| {
        let Some(ui) = weak.upgrade() else { return };
        let tint = if tinted { BorderTint::Artwork } else { BorderTint::Theme };
        // Ahead of the repaint, which reads it, and of the write, for the colour id's reason.
        TINT.set(tint);
        repaint_system(&ui);
        s.persist_blocking("persist window_border_tint", move |paths| {
            library::window::set_window_border_tint(paths, tint)
        });
    });
}

/// Repaint the outline if it is System's, whose colour moves without a palette change.
fn repaint_system(ui: &AppWindow) {
    if ui.global::<Settings>().get_window_border_system() {
        paint(ui, system_border(os_accent(), system_neutral(ui)));
    }
}

/// Seat, paint and persist `color_id`, System's or one of the theme's accents.
fn pick(ui: &AppWindow, state: &AppState, theme: &ThemeDef, color_id: &'static str) {
    let g = ui.global::<Settings>();
    let slot = accent_slot(theme, color_id);
    seat(&g, slot);
    // The swatch is the pick already shaded for the live variant, which saves resolving it.
    let picked = slot
        .and_then(|slot| g.get_accent_colors().row_data(slot))
        .map(|swatch| brush_to_rgb(&swatch));
    paint(ui, border_colors(picked, os_accent(), system_neutral(ui)));

    // Ahead of the write, so a palette change landing before it commits re-resolves this pick.
    COLOR_ID.with_borrow_mut(|id| color_id.clone_into(id));
    state.persist_blocking("persist window_border_color", move |paths| {
        library::window::set_window_border_color(paths, color_id.to_owned())
    });
}

/// The switch and the grid never both read as picked: System holds no slot.
fn seat(g: &Settings<'_>, slot: Option<usize>) {
    g.set_window_border_system(slot.is_none());
    g.set_window_border_color_idx(slot_index(slot));
}

fn paint(ui: &AppWindow, (focused, unfocused): (u32, u32)) {
    let chrome = ui.global::<WindowChrome>();
    chrome.set_border_color(brush(focused));
    chrome.set_border_color_unfocused(brush(unfocused));
}

/// System's neutral: the painted palette's, unless the tint asks for the theme's own while
/// Material You paints the artwork's.
fn system_neutral(ui: &AppWindow) -> u32 {
    match (TINT.get(), THEME_NEUTRAL.get()) {
        (BorderTint::Theme, Some(theme_neutral)) => theme_neutral,
        _ => palette_neutral(ui),
    }
}

/// The neutral border for the palette `Theme` is settling on, which mid-fade is not the blend the
/// global holds.
fn palette_neutral(ui: &AppWindow) -> u32 {
    if let Some(settled) = theme_paint::settled() {
        return neutral_border(settled.palette.mantle, settled.palette.text);
    }
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

#[cfg(test)]
#[path = "tests/window_border_tests.rs"]
mod tests;
