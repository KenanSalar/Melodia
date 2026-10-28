//! The appearance signals `apply()` merges into the painted brushes.

use super::palette::Palette;

/// `theme` mirrors the portal's `color-scheme`, and `kde_palette` the parsed `kdeglobals` a KDE
/// System variant paints from. `material_you` is no OS signal, but the palette and accent
/// `tasks::material_you` derived from the playing cover, riding the same merge rather than a
/// parallel pipeline.
#[derive(Debug, Clone)]
pub struct SystemColorState {
    pub theme: String,
    #[cfg(target_os = "linux")]
    pub kde_palette: Option<super::kde::KdeColorPalette>,
    /// `(palette, accent_hex)`.
    pub material_you: Option<(Palette, u32)>,
}

impl SystemColorState {
    /// Before the portal answers, and where no system appearance is surfaced.
    pub fn unknown() -> Self {
        Self {
            theme: "dark".to_owned(),
            #[cfg(target_os = "linux")]
            kde_palette: None,
            material_you: None,
        }
    }
}
