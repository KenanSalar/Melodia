//! The flag structs behind Settings ▸ Appearance and ▸ Window: window chrome, the tray, layout and
//! the backdrop.

use serde::{Deserialize, Serialize};

use melodia_platform::services::platform::desktop::{HostDesktop, is_kde_desktop};

/// The custom titlebar's caption glyphs. `Standard` is the chip reading "Windows", under the token
/// shipped files already hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarButtonStyle {
    #[default]
    Standard,
    Macos,
    Kde,
}

/// Which edge the captions sit on, close staying at the outer corner either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarButtonSide {
    #[default]
    Right,
    Left,
}

/// Whether a frameless window draws the 1 px outline an OS draws round its own frames. A token
/// rather than a `bool`, [`WindowFlags`] already sitting at clippy's `struct_excessive_bools` cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowBorder {
    #[default]
    Shown,
    Hidden,
}

/// What the System border colour mixes from under Material You; elsewhere both read the theme's.
/// A token for [`WindowBorder`]'s reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BorderTint {
    /// The artwork's palette, so the outline follows the track.
    #[default]
    Artwork,
    /// The theme's own, the palette Color Style None paints.
    Theme,
}

/// The [`WindowFlags::window_border_color`] that follows the OS's own border colour. Every other
/// value is an accent id of the active theme, and one the theme doesn't have reads as this.
pub const WINDOW_BORDER_SYSTEM_COLOR: &str = "system";

/// The two `titlebar_button_*` fields apply only under the custom titlebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowFlags {
    pub is_maximized: bool,
    #[serde(alias = "mini_player_pinned")]
    pub always_on_top: bool,
    pub use_native_titlebar: bool,
    pub titlebar_button_style: TitlebarButtonStyle,
    pub titlebar_button_side: TitlebarButtonSide,
    /// The miniplayer's captions. Its own field rather than `titlebar_button_style`, the
    /// miniplayer drawing them under the native titlebar too, where that style is inert.
    pub mini_player_button_style: TitlebarButtonStyle,
    /// Drawn wherever the window is frameless, the miniplayer under the native titlebar included.
    pub window_border: WindowBorder,
    pub window_border_color: String,
    pub window_border_tint: BorderTint,
}

impl WindowFlags {
    /// A fresh install's chrome. Under Plasma the native titlebar is Breeze's own, keep-above
    /// button and unfocused fade included, so a KDE install opens on it.
    pub(super) fn first_launch(desktop: HostDesktop) -> Self {
        Self { use_native_titlebar: desktop == HostDesktop::Kde, ..Self::default() }
    }
}

impl Default for WindowFlags {
    fn default() -> Self {
        Self {
            is_maximized: false,
            always_on_top: false,
            // Also what a file without the key reads, so KDE's default lives in `first_launch`.
            use_native_titlebar: false,
            titlebar_button_style: TitlebarButtonStyle::Standard,
            titlebar_button_side: TitlebarButtonSide::Right,
            mini_player_button_style: TitlebarButtonStyle::Standard,
            window_border: WindowBorder::Shown,
            window_border_color: WINDOW_BORDER_SYSTEM_COLOR.to_owned(),
            window_border_tint: BorderTint::Artwork,
        }
    }
}

/// The icon ships on and closing into it is opt-in. Boot reads `tray_enabled` once, so flipping it
/// restarts, and `close_to_tray` greys out wherever no tray came up.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayFlags {
    pub tray_enabled: bool,
    pub close_to_tray: bool,
}

impl Default for TrayFlags {
    fn default() -> Self {
        Self { tray_enabled: true, close_to_tray: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutFlags {
    pub sidebar_collapsed: bool,
    pub progress_state_layer: bool,
    /// KDE-only: under the native titlebar, tints the sidebar and bar toward `Theme.base` while
    /// unfocused, as Breeze fades its decoration. On under KDE for a fresh install and a file that
    /// predates the key alike.
    #[serde(default = "default_match_unfocused_to_system_bg")]
    pub match_unfocused_to_system_bg: bool,
}

impl LayoutFlags {
    /// A fresh install's layout. KDE opens on the native titlebar, and the tint is what makes that
    /// titlebar's unfocused fade reach the chrome beside it.
    pub(super) fn first_launch(desktop: HostDesktop) -> Self {
        Self { match_unfocused_to_system_bg: desktop == HostDesktop::Kde, ..Self::default() }
    }
}

impl Default for LayoutFlags {
    fn default() -> Self {
        Self {
            sidebar_collapsed: false,
            progress_state_layer: true,
            match_unfocused_to_system_bg: false,
        }
    }
}

fn default_match_unfocused_to_system_bg() -> bool {
    is_kde_desktop()
}

/// The queue panel as the window closed, so a launch reopens it. Its own struct rather than more
/// fields on [`LayoutFlags`], which is already at clippy's `struct_excessive_bools` budget.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueuePanelLayout {
    /// What the user asked for, not whether it was on screen: a panel stepped aside under Now
    /// Playing or a narrow window comes back with the room.
    pub queue_panel_open: bool,
    /// `None` until the edge is dragged, so a panel never resized follows `Theme`'s default.
    pub queue_panel_width: Option<f64>,
}

/// Which backdrop the artwork-derived surfaces paint, its own struct for [`QueuePanelLayout`]'s
/// reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackdropFlags {
    /// Restart-gated, read once at boot into `Theme.aurora-backdrop`. On, matching `theme.slint`'s
    /// own value so a failed settings read lands on the shipped look.
    pub aurora_backdrop: bool,
    /// Whether the miniplayer paints that backdrop too. Live, since it only mounts one more
    /// surface over the stack already chosen; off, so an install keeps the miniplayer it has.
    pub mini_backdrop: bool,
}

impl Default for BackdropFlags {
    fn default() -> Self {
        Self { aurora_backdrop: true, mini_backdrop: false }
    }
}
