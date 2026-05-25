//! OS appearance signals consumed by `apply()`. Carries the XDG portal
//! `color-scheme` value, the parsed KDE `kdeglobals` palette (Linux only),
//! and the latest Material You dynamic palette + accent generated from the
//! currently-playing track's album art by `tasks::material_you`.

use super::palette::Palette;

/// Bundled OS appearance signals consumed by `apply()`. `theme` mirrors the
/// XDG portal `color-scheme` value (`"dark"` / `"light"`); on Linux KDE
/// sessions `kde_palette` carries the parsed `kdeglobals` colours so the
/// KDE Breeze theme can override its static Light/Dark palette with live
/// OS colours when the user picks the System variant.
///
/// `material_you` is **not** an OS signal — it's the latest palette + accent
/// generated from the currently-playing track's album art by
/// `tasks::material_you`. Lives here so the same `apply()` plumbing that
/// already merges OS signals into the painted brushes can also merge in
/// the dynamic palette without inventing a parallel pipeline.
#[derive(Debug, Clone)]
pub struct SystemColorState {
    pub theme: String,
    #[cfg(target_os = "linux")]
    pub kde_palette: Option<crate::services::system_theme::KdeColorPalette>,
    /// `(palette, accent_hex)` produced by the Material You coordinator.
    /// `Palette` is `Copy` (32 bytes), so this slot is essentially free
    /// to clone alongside the rest of the state.
    pub material_you: Option<(Palette, u32)>,
}

impl SystemColorState {
    /// Conservative default used at startup before the XDG portal answers
    /// and on platforms that don't surface a system appearance signal.
    pub fn unknown() -> Self {
        Self {
            theme: "dark".to_owned(),
            #[cfg(target_os = "linux")]
            kde_palette: None,
            material_you: None,
        }
    }
}
