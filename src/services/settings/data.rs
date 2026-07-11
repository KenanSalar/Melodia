//! `settings.json` data model: every serde struct that makes up
//! [`SettingsData`], their first-launch defaults, and the OS / desktop
//! environment probes those defaults depend on. The load / save / mutate
//! I/O lives in the sibling [`super::io`] module.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::player::crossfade::DEFAULT_CROSSFADE_MS;
use crate::player::equalizer::{DEFAULT_PRESET, NUM_BANDS};
use crate::player::replaygain::{DEFAULT_MODE, RG_DEFAULT_PREAMP_DB};
use crate::player::types::RepeatMode;

pub const MAX_CORNER_RADIUS: u32 = 15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemePreference {
    pub variant: String,
    pub accent: String,
    /// Last accent the user picked that was *not* `MATERIAL_YOU_ACCENT_ID`.
    /// Drives two UX paths: (1) when Material You is the active accent but
    /// no dynamic palette is available (no current artwork / extraction
    /// failed), the dot grid + painted accent fall back to this instead of
    /// the theme's hard default; (2) when the user disables Color Style
    /// (None), the persisted accent is restored to this value rather than
    /// jumping to the theme default. Optional + `serde(default)` keeps
    /// older settings.json files readable.
    #[serde(default)]
    pub last_static_accent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
        if token == "desc" {
            SortDir::Desc
        } else {
            SortDir::Asc
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSort {
    pub field: String,
    pub dir: SortDir,
}

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
        // Match the Slint side initial values in `ui/globals.slint` —
        // the GTK-FIXED column model needs `title` to have a real width
        // (Tauri's flex-cap model didn't).
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

/// Audio-playback preferences persisted with the rest of `SettingsData`.
/// Grouped into a substruct so each field still serializes at the top level
/// of the JSON file (`#[serde(flatten)]` on the parent field) while keeping
/// the `clippy::struct_excessive_bools` budget per struct manageable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackFlags {
    pub gapless_playback: bool,
    pub resume_on_startup: bool,
    pub is_muted: bool,
    /// Playback-rate multiplier (1.0 = normal). Clamped to the player's
    /// `MIN_SPEED..=MAX_SPEED` range when applied / persisted. The struct's
    /// `#[serde(default)]` makes older `settings.json` files (written before
    /// this field existed) deserialize it to `1.0`.
    pub playback_speed: f64,
}

impl Default for PlaybackFlags {
    fn default() -> Self {
        Self {
            gapless_playback: true,
            resume_on_startup: false,
            is_muted: false,
            playback_speed: 1.0,
        }
    }
}

/// Graphic-equalizer preferences. Like the other audio substructs this is
/// `#[serde(flatten)]`'d into `SettingsData` so each field serializes at the
/// top level of `settings.json`, and `#[serde(default)]` makes older files
/// (written before the EQ existed) deserialize to the neutral defaults.
///
/// Defaults are deliberately inert: the EQ ships **off** with a flat curve, so
/// a fresh install — or any older `settings.json` — sounds bit-identical to no
/// EQ until the user opts in. Gains are stored in dB; the loader
/// (`equalizer::normalize_gains`) clamps the range and fixes the list length on
/// read, so a hand-edited or wrong-length array can't pin a bad value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EqualizerFlags {
    pub eq_enabled: bool,
    pub eq_band_gains: Vec<f32>,
    pub eq_selected_preset: String,
    /// Preamp / master gain in dB (0 = unity). Clamped to the player's
    /// `MIN_PREAMP_DB..=MAX_PREAMP_DB` range when applied / persisted; the
    /// `#[serde(default)]` makes older files deserialize it to `0.0`.
    pub eq_preamp: f32,
}

impl Default for EqualizerFlags {
    fn default() -> Self {
        Self {
            eq_enabled: false,
            eq_band_gains: vec![0.0; NUM_BANDS],
            eq_selected_preset: DEFAULT_PRESET.to_owned(),
            eq_preamp: 0.0,
        }
    }
}

/// `ReplayGain` (loudness normalization) preferences. Like the other audio
/// substructs this is `#[serde(flatten)]`'d into `SettingsData` and carries a
/// whole-struct `#[serde(default)]` so older `settings.json` files (written
/// before `ReplayGain` existed) deserialize to these neutral defaults.
///
/// Defaults are inert: `ReplayGain` ships **off**, so a fresh install — or any
/// older file — plays at the raw recorded level until the user opts in. The
/// `rg_prevent_clipping` guard defaults **on** so a boosted track never clips
/// once `ReplayGain` is enabled. `rg_mode` is stored as a lowercase token
/// (`"track"` / `"album"`); the loader falls back to Album on an unknown value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayGainFlags {
    pub rg_enabled: bool,
    pub rg_mode: String,
    /// Extra preamp in dB (0 = unity), clamped to the player's
    /// `RG_MIN_PREAMP_DB..=RG_MAX_PREAMP_DB` range when applied / persisted.
    pub rg_preamp: f32,
    pub rg_prevent_clipping: bool,
}

impl Default for ReplayGainFlags {
    fn default() -> Self {
        Self {
            rg_enabled: false,
            rg_mode: DEFAULT_MODE.to_owned(),
            rg_preamp: RG_DEFAULT_PREAMP_DB,
            rg_prevent_clipping: true,
        }
    }
}

/// Crossfade preferences. Like the other audio substructs this is
/// `#[serde(flatten)]`'d into `SettingsData` and carries a whole-struct
/// `#[serde(default)]`, so older `settings.json` files (written before
/// crossfade existed) deserialize to these defaults.
///
/// Defaults are inert: crossfade ships **off**, so a fresh install — or any
/// older file — keeps the existing gapless behaviour untouched. When the user
/// does enable it, `crossfade_skip_same_album` defaults **on** so continuous-mix
/// albums stay gapless; that mirrors Strawberry's and Clementine's defaults.
/// `crossfade_duration_ms` is clamped to the player's
/// `MIN_CROSSFADE_MS..=MAX_CROSSFADE_MS` range when applied / persisted.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one serde field per independent user-facing toggle; each must round-trip through settings.json by name"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CrossfadeFlags {
    pub crossfade_enabled: bool,
    pub crossfade_duration_ms: u32,
    pub crossfade_manual: bool,
    pub crossfade_skip_same_album: bool,
    pub crossfade_fade_on_pause: bool,
}

impl Default for CrossfadeFlags {
    fn default() -> Self {
        Self {
            crossfade_enabled: false,
            crossfade_duration_ms: DEFAULT_CROSSFADE_MS,
            crossfade_manual: false,
            crossfade_skip_same_album: true,
            crossfade_fade_on_pause: false,
        }
    }
}

/// Queue-behavior preferences. Split out from `PlaybackFlags` so each
/// substruct stays under the `clippy::struct_excessive_bools` budget
/// (≤3 bools). Like the other substructs, this is `#[serde(flatten)]`'d
/// into `SettingsData` so each field still serializes at the top level
/// of `settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueFlags {
    pub shuffle_enabled: bool,
    pub repeat_mode: RepeatMode,
}

impl Default for QueueFlags {
    fn default() -> Self {
        Self {
            shuffle_enabled: false,
            repeat_mode: RepeatMode::Off,
        }
    }
}

/// Style of the three minimize / maximize / close decoration buttons in
/// the custom titlebar. `Standard` paints the existing Material Symbols
/// bar icons (Windows / KDE convention); `Macos` paints the three
/// traffic-light circles (red close, yellow minimize, green maximize)
/// with hover-reveal glyphs and a grey-when-unfocused fill. Persisted as
/// a `snake_case` token in `settings.json` so future styles can be added
/// without a breaking schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarButtonStyle {
    #[default]
    Standard,
    Macos,
}

/// Which window edge the custom titlebar's decoration buttons sit on.
/// Independent of `TitlebarButtonStyle` — the four combinations all
/// render cleanly, with close kept at the outer window corner regardless
/// (so muscle-memory "click corner = close" holds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarButtonSide {
    #[default]
    Right,
    Left,
}

/// Window-chrome toggles. See `PlaybackFlags` for the substruct rationale.
/// `titlebar_button_style` + `titlebar_button_side` only take effect when
/// `use_native_titlebar == false` — under the native titlebar the OS
/// paints its own decoration buttons.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowFlags {
    pub is_maximized: bool,
    #[serde(alias = "mini_player_pinned")]
    pub always_on_top: bool,
    pub use_native_titlebar: bool,
    pub titlebar_button_style: TitlebarButtonStyle,
    pub titlebar_button_side: TitlebarButtonSide,
}

impl Default for WindowFlags {
    fn default() -> Self {
        Self {
            is_maximized: false,
            always_on_top: false,
            // Windows: with `DWMWA_USE_IMMERSIVE_DARK_MODE` +
            // `DWMWA_CAPTION_COLOR` painting the OS caption in the app
            // mantle, the native titlebar is visually near-identical to
            // the custom one — and gains real Aero Snap, hover-peek,
            // and Windows-shell consistency. Default to the native
            // chrome there.
            //
            // Linux + macOS keep the custom titlebar by default: it's
            // the only chrome that integrates the playback controls
            // and Now Playing strip without losing rows to a separate
            // OS frame, and KDE / GNOME / macOS users get visual parity
            // with apps like Spotify / VLC / Vinyl that ship their own
            // window chrome.
            use_native_titlebar: cfg!(target_os = "windows"),
            titlebar_button_style: TitlebarButtonStyle::Standard,
            titlebar_button_side: TitlebarButtonSide::Right,
        }
    }
}

/// System-tray toggles. A dedicated substruct (rather than a fourth bool on
/// `WindowFlags`) keeps each struct under the `clippy::struct_excessive_bools`
/// budget; `#[serde(flatten)]`'d into `SettingsData` so the field still
/// serializes at the top level of `settings.json`.
///
/// `tray_enabled` defaults to `false` — the tray icon is opt-in. When off,
/// `main.rs` skips `ui::tray_bridge::install` entirely, so none of the tray
/// code runs: no D-Bus connection, no service thread, no action tasks.
/// Toggling it requires a restart (the `restart-tray` `Dialog` flow).
///
/// `close_to_tray` defaults to `false` — closing the window quits the app,
/// matching every release before the tray landed. When `true` (and a tray
/// icon is actually active) a window-close hides to tray instead; quit is
/// then reached through the tray menu.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayFlags {
    pub tray_enabled: bool,
    pub close_to_tray: bool,
}

/// Library-management toggles. See `PlaybackFlags` for the substruct rationale.
///
/// `folder_watching_enabled` defaults to `true` — consumer music players
/// (Apple Music, Groove, Windows Media Player, Spotify) all auto-watch with
/// no toggle, and a new install with watching off lands in a stale-UI
/// failure mode the user can't easily diagnose. The toggle stays for the
/// inotify-watch-budget escape valve on huge libraries, but it's opt-out
/// now rather than opt-in. Existing `settings.json` files with an explicit
/// `false` keep that value (serde reads the field as-written).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryFlags {
    pub folder_watching_enabled: bool,
    pub music_folder_auto_added: bool,
}

impl Default for LibraryFlags {
    fn default() -> Self {
        Self {
            folder_watching_enabled: true,
            music_folder_auto_added: false,
        }
    }
}

/// Auto-updater state persisted between launches. See `PlaybackFlags` for
/// the substruct rationale.
///
/// `last_check_unix` and `last_manifest_etag` drive the daily-check loop's
/// 24h elapsed gate + `If-None-Match` short-circuit. `consecutive_failures`
/// is incremented on every fetch error (saturating at `u8::MAX`); once it
/// reaches 3 the loop backs off from a 6h re-arm to a 7d re-arm, resetting
/// to 0 on the next successful check (mitigates flaky-network / firewall
/// thrash). `skipped_release` is set only when the user clicks the
/// "Skip this version" affordance in Settings → Updates — closing the
/// notification toast does NOT skip; that distinction matters because a
/// dismissed toast re-appears on next launch while a skipped version is
/// suppressed until a strictly-newer version lands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateFlags {
    pub auto_check_enabled: bool,
    pub last_check_unix: i64,
    pub last_known_release: String,
    pub skipped_release: String,
    pub last_manifest_etag: String,
    pub consecutive_failures: u8,
}

impl Default for UpdateFlags {
    fn default() -> Self {
        Self {
            auto_check_enabled: true,
            last_check_unix: 0,
            last_known_release: String::new(),
            skipped_release: String::new(),
            last_manifest_etag: String::new(),
            consecutive_failures: 0,
        }
    }
}

/// In-app layout/visual toggles. See `PlaybackFlags` for the substruct rationale.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutFlags {
    pub sidebar_collapsed: bool,
    pub progress_state_layer: bool,
    /// KDE-only visual toggle: when true and the native titlebar is
    /// active, the sidebar and now-playing bar tint toward
    /// `Theme.base` while the window is unfocused (mirrors KDE's
    /// window-decoration fade). `#[serde(default)]` calls
    /// `default_match_unfocused_to_system_bg` so older
    /// `settings.json` files (no key) seed `true` on KDE and `false`
    /// everywhere else; `LayoutFlags::default()` keeps the field
    /// `false` so a fresh non-KDE install boots with the row hidden
    /// and the feature off.
    #[serde(default = "default_match_unfocused_to_system_bg")]
    pub match_unfocused_to_system_bg: bool,
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

// ----- OS / desktop-environment helpers -----
//
// These read process env vars (Linux) or evaluate compile-time `cfg` flags
// (macOS / Windows) to derive sensible OS-aware defaults for settings
// fields. They live in the services layer because (a) they're called
// from `SettingsData::default()` and `default_match_unfocused_to_system_bg`,
// both inside this file, and (b) the `library/` layer should depend on
// `services/`, not the other way around. UI code that needs them
// (`src/ui/appearance.rs`) imports from `services::settings::` directly.

/// True iff the active session is KDE Plasma — read from
/// `$XDG_CURRENT_DESKTOP`. Drives the default seed for
/// `LayoutFlags.match_unfocused_to_system_bg` and the visibility of
/// the matching toggle in the Appearance section (we hide the row
/// off-KDE because the behaviour mirrors KDE's window-decoration
/// fade and isn't meaningful on GNOME / Windows / macOS).
#[cfg(target_os = "linux")]
pub fn is_kde_desktop() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .is_some_and(|val| val.split(':').any(|seg| seg == "KDE"))
}

#[cfg(not(target_os = "linux"))]
pub fn is_kde_desktop() -> bool {
    false
}

/// Returns the OS/DE-appropriate window corner radius in pixels.
/// Mirrors the chip presets exposed in the Appearance section so the
/// first-launch default lights up the chip that matches the host
/// environment (Windows 11 = 8, macOS = 10, GNOME = 15, KDE = 6,
/// other Linux desktops fall back to 6).
#[must_use]
pub fn get_os_corner_radius() -> u32 {
    #[cfg(target_os = "macos")]
    {
        10
    }
    #[cfg(target_os = "windows")]
    {
        8
    }
    #[cfg(target_os = "linux")]
    {
        match std::env::var("XDG_CURRENT_DESKTOP") {
            Ok(val) => {
                for segment in val.split(':') {
                    if segment == "GNOME" {
                        return 15;
                    }
                    if segment == "KDE" {
                        return 6;
                    }
                }
                6
            }
            Err(_) => 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsData {
    pub theme_id: String,
    pub theme_variant: String,
    pub accent_color: String,
    pub sidebar_width: f64,
    pub window_width: f64,
    pub window_height: f64,
    pub window_x: f64,
    pub window_y: f64,
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
    pub queue: QueueFlags,
    #[serde(flatten)]
    pub window: WindowFlags,
    #[serde(flatten)]
    pub tray: TrayFlags,
    #[serde(flatten)]
    pub library: LibraryFlags,
    #[serde(flatten)]
    pub layout: LayoutFlags,
    #[serde(flatten)]
    pub updates: UpdateFlags,
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            theme_id: "catppuccin".to_owned(),
            theme_variant: "mocha".to_owned(),
            accent_color: "mauve".to_owned(),
            sidebar_width: 180.0,
            window_width: 1200.0,
            window_height: 800.0,
            window_x: 100.0,
            window_y: 100.0,
            volume: 100,
            // First-launch default tracks the host OS / desktop so the
            // window outline feels native out of the box: Windows 11 = 8,
            // macOS = 10, GNOME = 15, KDE = 6, other Linux desktops fall
            // back to 6. Mirrors the preset chips in the Settings page.
            // Existing installs already have a `corner_radius` field in
            // their `settings.json` so this default only fires on a fresh
            // boot (no settings file) or if the field is missing —
            // returning users keep whatever value they previously chose.
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
            queue: QueueFlags::default(),
            window: WindowFlags::default(),
            tray: TrayFlags::default(),
            library: LibraryFlags::default(),
            layout: LayoutFlags::default(),
            updates: UpdateFlags::default(),
        }
    }
}

/// Locale codes the bundled `.po` files cover (`translations/<code>/LC_MESSAGES/Melodia.po`).
/// Index 0 is the canonical default — its msgid baseline is English literals, so no
/// `en.po` is shipped (English is the source language and lives in `.slint` directly).
/// Order is the display order rendered in the Language section dropdown.
pub const SUPPORTED_LOCALES: &[&str] = &["en", "de", "fr", "es", "tr", "el", "it"];

fn default_locale() -> String {
    detect_os_locale().unwrap_or_else(|| "en".to_owned())
}

fn detect_os_locale() -> Option<String> {
    let raw = detect_system_locale_raw()?;
    let lang = parse_language_code(&raw)?;
    if SUPPORTED_LOCALES.contains(&lang.as_str()) {
        Some(lang)
    } else {
        None
    }
}

fn detect_system_locale_raw() -> Option<String> {
    if let Ok(val) = std::env::var("LANGUAGE")
        && !val.is_empty()
        && let Some(first) =
            val.split(':').find(|s| !s.is_empty() && *s != "C" && *s != "POSIX")
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
    reason = "FFI declaration + call for GetUserDefaultLocaleName; writes into a stack-sized [u16] buffer, length-bounded by GetUserDefaultLocaleName's i32 return"
)]
fn detect_windows_locale() -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    unsafe extern "system" {
        fn GetUserDefaultLocaleName(lpLocaleName: *mut u16, cchLocaleName: i32) -> i32;
    }

    // Buffer is `[u16; 85]` — `i32::try_from(85)` is infallible; the
    // `unwrap_or` keeps the call lint-clean without an `unwrap()`.
    let mut buf = [0u16; 85];
    let buf_len = i32::try_from(buf.len()).unwrap_or(i32::MAX);
    let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf_len) };
    if len > 0 {
        // `len` is positive here, so `usize::try_from` succeeds; the
        // saturating sub drops the trailing NUL.
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
    if lang.len() == 2 && lang.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(lang)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../tests/settings_tests.rs"]
mod tests;
