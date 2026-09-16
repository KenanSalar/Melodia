//! The flag structs behind the Settings ▸ Interface tab: window chrome, the tray, layout, motion and
//! the backdrop.

use serde::{Deserialize, Serialize};

use melodia_platform::services::platform::desktop::{HostDesktop, is_kde_desktop};

/// Style of the custom titlebar's decoration buttons: `Standard` paints
/// Windows 11's caption glyphs, `Macos` the three traffic-light circles,
/// `Kde` Breeze's glyphs. Persisted as a token so a future style needs no
/// schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarButtonStyle {
    #[default]
    Standard,
    Macos,
    Kde,
}

/// Which window edge the decoration buttons sit on. Independent of
/// [`TitlebarButtonStyle`]; close stays at the outer corner either way, so
/// "click corner = close" holds.
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

/// The [`WindowFlags::window_border_color`] that follows the OS's own border colour. Every other
/// value is an accent id of the active theme, and one the theme doesn't have reads as this.
pub const WINDOW_BORDER_SYSTEM_COLOR: &str = "system";

/// Window-chrome toggles. The two `titlebar_button_*` fields only take effect
/// under `use_native_titlebar == false`; otherwise the OS paints its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowFlags {
    pub is_maximized: bool,
    #[serde(alias = "mini_player_pinned")]
    pub always_on_top: bool,
    pub use_native_titlebar: bool,
    pub titlebar_button_style: TitlebarButtonStyle,
    pub titlebar_button_side: TitlebarButtonSide,
    /// The square miniplayer's captions. Its own field rather than `titlebar_button_style`, the
    /// miniplayer drawing them under the native titlebar too, where that style is inert.
    pub mini_player_button_style: TitlebarButtonStyle,
    /// Drawn wherever the window is frameless: always under the custom titlebar, and for the
    /// miniplayer under the native one.
    pub window_border: WindowBorder,
    pub window_border_color: String,
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
            // Also what a file without the key reads as, so it stays off here and
            // `first_launch` answers KDE. On Windows the native frame, painted in the app mantle
            // through the DWM caption attributes, stays a Settings toggle.
            use_native_titlebar: false,
            titlebar_button_style: TitlebarButtonStyle::Standard,
            titlebar_button_side: TitlebarButtonSide::Right,
            mini_player_button_style: TitlebarButtonStyle::Standard,
            window_border: WindowBorder::Shown,
            window_border_color: WINDOW_BORDER_SYSTEM_COLOR.to_owned(),
        }
    }
}

/// System-tray toggles. The icon ships on; hiding the window into it is opt-in.
///
/// With `tray_enabled` off, `ui::shell::tray_bridge::install` is skipped
/// entirely — no D-Bus connection, no service thread, no action tasks — so
/// toggling it needs a restart (the `restart-tray` `Dialog` flow). A session
/// with no tray host still pays that install and leaves `Settings.tray-active`
/// false, which is what greys `close_to_tray` out: with no icon the close
/// handlers quit regardless, so it can never strand the window.
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

/// In-app layout / visual toggles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutFlags {
    pub sidebar_collapsed: bool,
    pub progress_state_layer: bool,
    /// KDE-only: under the native titlebar, tint the sidebar and now-playing
    /// bar toward `Theme.base` while unfocused, mirroring KDE's own
    /// window-decoration fade. On under KDE both for a fresh install
    /// ([`LayoutFlags::first_launch`]) and for a file that predates the key (the field default).
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

/// Motion the shell plays for its own sake.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionFlags {
    /// Drop the entrance animation of the view mounted at launch, so the window
    /// opens with its content already in place. `boot::ui_setup` turns it into
    /// `Nav.suppress-enter-animation` before the window is shown and the mount
    /// hands that back once settled; later navigation animates either way.
    pub skip_startup_animation: bool,
}

/// Which backdrop the artwork-derived surfaces paint.
///
/// Its own struct rather than a fourth bool on [`LayoutFlags`], which is already at clippy's
/// `struct_excessive_bools` budget. Read once at boot into `Theme.aurora-backdrop` — the toggle is
/// restart-gated, so the artwork tiers can decide whether to build a blur half at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackdropFlags {
    /// On, and `theme.slint` declares the same value so a failed settings read lands on the
    /// shipped look.
    pub aurora_backdrop: bool,
    /// Whether the miniplayer paints that backdrop too, instead of flat chrome.
    ///
    /// **A different question from the one above, which is why it is not restart-gated.** That one
    /// picks which of the two stacks every colour tier is solved for and so what the artwork tiers
    /// build per decode; this only says whether one more surface mounts the stack already chosen.
    /// Off, so an install that never presses the button keeps the miniplayer it has.
    pub mini_backdrop: bool,
}

impl Default for BackdropFlags {
    fn default() -> Self {
        Self { aurora_backdrop: true, mini_backdrop: false }
    }
}
