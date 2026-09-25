//! `settings.json`'s root model, [`SettingsData`], with the view primitives it stores and the
//! first-launch defaults no flag struct owns: the host's theme and the OS locale.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::about::{DiagnosticsFlags, OnboardingFlags, SupportFlags, UpdateFlags};
use super::interface::{BackdropFlags, LayoutFlags, MotionFlags, TrayFlags, WindowFlags};
use super::library::LibraryFlags;
use super::playback::{
    CrossfadeFlags, EqualizerFlags, OutputFlags, PlaybackFlags, QueueFlags, ReplayGainFlags,
    VisualizerFlags,
};
use super::services::{LyricsFlags, RadioFlags};
use melodia_core::entities::integrations::{DiscordFlags, ScrobbleFlags};
use melodia_core::entities::locale::SUPPORTED_LOCALES;
use melodia_core::themes::{self, SYSTEM_VARIANT_ID, ThemeDef};
use melodia_platform::services::platform::desktop::{
    HostDesktop, get_os_corner_radius, host_desktop,
};

pub const MAX_CORNER_RADIUS: u32 = 15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemePreference {
    pub variant: String,
    pub accent: String,
    /// Last accent picked that was *not* `MATERIAL_YOU_ACCENT_ID`. Two paths
    /// fall back to it rather than to the theme's hard default: Material You
    /// with no dynamic palette available, and disabling Color Style outright.
    #[serde(default)]
    pub last_static_accent: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

impl SortDir {
    /// The lowercase token the Slint `sort-dir` properties use.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            SortDir::Asc => "asc",
            SortDir::Desc => "desc",
        }
    }

    /// Parse a Slint `sort-dir` token; anything other than `"desc"` is `Asc`.
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        if token == "desc" { SortDir::Desc } else { SortDir::Asc }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSort {
    pub field: String,
    pub dir: SortDir,
}

/// One track list's stored column widths. `number`, `year` and `length` are pixels; the other
/// four are the width each had when last sized, read as weights against whatever room the list
/// has, so a saved layout keeps its proportions at any window size. `melodia-views`'
/// `ui::track_columns` is the only reader of either meaning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnWidths {
    pub number: f64,
    pub title: f64,
    pub artist: f64,
    pub album: f64,
    pub genre: f64,
    pub year: f64,
    pub length: f64,
}

impl Default for ColumnWidths {
    fn default() -> Self {
        // Every track list's first-launch widths; the Slint globals declare none.
        Self {
            number: 56.0,
            title: 320.0,
            artist: 200.0,
            album: 220.0,
            genre: 140.0,
            year: 72.0,
            length: 88.0,
        }
    }
}

/// A window's top-left corner in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPosition {
    pub x: f64,
    pub y: f64,
}

/// The full player the miniplayer's restore caption hands back, kept across a close from the
/// miniplayer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FullPlayerGeometry {
    pub width: f64,
    pub height: f64,
    /// `None` where the window never learned its own position, Wayland keeping it to itself.
    pub position: Option<WindowPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsData {
    pub theme_id: String,
    pub theme_variant: String,
    pub accent_color: String,
    pub sidebar_width: f64,
    /// The window as it closed, the miniplayer included.
    pub window_width: f64,
    pub window_height: f64,
    pub window_x: f64,
    pub window_y: f64,
    /// `Some` only when the window closed as the miniplayer.
    pub full_player_geometry: Option<FullPlayerGeometry>,
    pub volume: u32,
    pub corner_radius: u32,
    pub play_button_animation: String,
    pub dynamic_color_style: String,
    #[serde(default)]
    pub theme_preferences: HashMap<String, ThemePreference>,
    #[serde(default)]
    pub overflow_buttons: Vec<String>,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(flatten)]
    pub playback: PlaybackFlags,
    #[serde(flatten)]
    pub equalizer: EqualizerFlags,
    #[serde(flatten)]
    pub replaygain: ReplayGainFlags,
    #[serde(flatten)]
    pub crossfade: CrossfadeFlags,
    #[serde(flatten)]
    pub output: OutputFlags,
    #[serde(flatten)]
    pub visualizer: VisualizerFlags,
    #[serde(flatten)]
    pub queue: QueueFlags,
    #[serde(flatten)]
    pub window: WindowFlags,
    #[serde(flatten)]
    pub tray: TrayFlags,
    #[serde(flatten)]
    pub scrobble: ScrobbleFlags,
    #[serde(flatten)]
    pub discord: DiscordFlags,
    #[serde(flatten)]
    pub radio: RadioFlags,
    #[serde(flatten)]
    pub lyrics: LyricsFlags,
    #[serde(flatten)]
    pub library: LibraryFlags,
    #[serde(flatten)]
    pub layout: LayoutFlags,
    #[serde(flatten)]
    pub motion: MotionFlags,
    #[serde(flatten)]
    pub backdrop: BackdropFlags,
    #[serde(flatten)]
    pub updates: UpdateFlags,
    #[serde(flatten)]
    pub diagnostics: DiagnosticsFlags,
    #[serde(flatten)]
    pub support: SupportFlags,
    #[serde(flatten)]
    pub onboarding: OnboardingFlags,
}

impl SettingsData {
    /// The static accent `theme_id` last had, if one was ever recorded.
    pub fn last_static_accent(&self, theme_id: &str) -> Option<&str> {
        self.theme_preferences.get(theme_id)?.last_static_accent.as_deref()
    }
}

impl Default for SettingsData {
    fn default() -> Self {
        let desktop = host_desktop();
        let (theme, variant) = first_launch_theme(desktop);
        Self {
            theme_id: theme.id.to_owned(),
            theme_variant: variant.to_owned(),
            accent_color: theme.default_accent.to_owned(),
            sidebar_width: 180.0,
            window_width: 1200.0,
            window_height: 800.0,
            window_x: 100.0,
            window_y: 100.0,
            full_player_geometry: None,
            volume: 100,
            // Tracks the host desktop so the window outline feels native out of
            // the box; a returning install already has the field written.
            corner_radius: get_os_corner_radius(),
            play_button_animation: "none".to_owned(),
            dynamic_color_style: "none".to_owned(),
            theme_preferences: HashMap::new(),
            overflow_buttons: Vec::new(),
            locale: default_locale(),
            playback: PlaybackFlags::default(),
            equalizer: EqualizerFlags::default(),
            replaygain: ReplayGainFlags::default(),
            crossfade: CrossfadeFlags::default(),
            output: OutputFlags::default(),
            visualizer: VisualizerFlags::default(),
            queue: QueueFlags::default(),
            window: WindowFlags::first_launch(desktop),
            tray: TrayFlags::default(),
            scrobble: ScrobbleFlags::default(),
            discord: DiscordFlags::default(),
            radio: RadioFlags::default(),
            lyrics: LyricsFlags::default(),
            library: LibraryFlags::default(),
            layout: LayoutFlags::first_launch(desktop),
            motion: MotionFlags::default(),
            backdrop: BackdropFlags::default(),
            updates: UpdateFlags::default(),
            diagnostics: DiagnosticsFlags::default(),
            support: SupportFlags::default(),
            onboarding: OnboardingFlags::default(),
        }
    }
}

/// The theme and variant a fresh install opens with, its accent being the theme's own default.
/// Windows, KDE and GNOME each open on their own theme under the System variant, so the first
/// window matches the desktop around it. A desktop with no theme of its own gets Catppuccin.
fn first_launch_theme(desktop: HostDesktop) -> (&'static ThemeDef, &'static str) {
    if cfg!(target_os = "windows") {
        return (&themes::windows::WINDOWS, SYSTEM_VARIANT_ID);
    }
    match desktop {
        HostDesktop::Kde => (&themes::kde::KDE, SYSTEM_VARIANT_ID),
        HostDesktop::Gnome => (&themes::gnome::GNOME, SYSTEM_VARIANT_ID),
        HostDesktop::Other => {
            (&themes::catppuccin::CATPPUCCIN, themes::catppuccin::CATPPUCCIN.default_variant)
        }
    }
}

fn default_locale() -> String {
    detect_os_locale().unwrap_or_else(|| "en".to_owned())
}

fn detect_os_locale() -> Option<String> {
    let raw = detect_system_locale_raw()?;
    let lang = parse_language_code(&raw)?;
    if SUPPORTED_LOCALES.contains(&lang.as_str()) { Some(lang) } else { None }
}

fn detect_system_locale_raw() -> Option<String> {
    if let Ok(val) = std::env::var("LANGUAGE")
        && !val.is_empty()
        && let Some(first) = val.split(':').find(|s| !s.is_empty() && *s != "C" && *s != "POSIX")
    {
        return Some(first.to_owned());
    }

    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(val) = std::env::var(var)
            && !val.is_empty()
            && val != "C"
            && val != "POSIX"
        {
            return Some(val);
        }
    }

    #[cfg(target_os = "windows")]
    {
        return detect_windows_locale();
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "FFI call to GetUserDefaultLocaleName; writes into a stack-sized [u16] buffer, bounded by the cchLocaleName it is handed"
)]
fn detect_windows_locale() -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    // The conversion is infallible; the `unwrap_or` keeps it lint-clean and
    // saturates *down*, so the unreachable arm claims a smaller buffer than
    // there is rather than a larger one.
    let mut buf = [0u16; 85];
    let buf_len = i32::try_from(buf.len()).unwrap_or(0);
    // SAFETY: `buf_len` never exceeds `buf`'s length, so the pointer is valid for
    // every `u16` the call may write.
    let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf_len) };
    if len > 0 {
        // The saturating sub drops the trailing NUL.
        let actual_len = usize::try_from(len).unwrap_or(0).saturating_sub(1);
        if actual_len > buf.len() {
            return None;
        }
        let os_str = OsString::from_wide(&buf[..actual_len]);
        os_str.to_str().map(str::to_owned)
    } else {
        None
    }
}

fn parse_language_code(locale_str: &str) -> Option<String> {
    let without_encoding = locale_str.split('.').next()?;
    let lang = without_encoding.split(['_', '-']).next()?;
    let lang = lang.to_lowercase();
    if lang.len() == 2 && lang.chars().all(|c| c.is_ascii_alphabetic()) { Some(lang) } else { None }
}

#[cfg(test)]
#[path = "../tests/settings_tests.rs"]
mod tests;
