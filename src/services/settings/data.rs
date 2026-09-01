//! `settings.json` data model: every serde struct that makes up
//! [`SettingsData`], their first-launch defaults, and the OS / desktop probes
//! those defaults depend on. The load / save / mutate I/O is [`super::io`].
//!
//! Every `*Flags` substruct here follows one shape: `#[serde(flatten)]`'d into
//! [`SettingsData`] so its fields still serialize at the top level of the file,
//! and carrying a whole-struct `#[serde(default)]` so an install written before
//! the feature loads anyway. The grouping exists to keep each struct under
//! clippy's `struct_excessive_bools` budget; the flatten is what keeps that
//! free of on-disk consequences. Per the shipped-app rule, anything with new
//! visible behavior defaults off.

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
        // Mirrors the initial values in `melodia-ui/ui/globals/tracks.slint`;
        // the fixed column model needs `title` to carry a real width.
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

/// Audio-playback preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackFlags {
    pub gapless_playback: bool,
    pub resume_on_startup: bool,
    pub is_muted: bool,
    /// Rate multiplier, clamped to the player's `MIN_SPEED..=MAX_SPEED` when
    /// applied or persisted.
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

/// Graphic-equalizer preferences. Ships **off** with a flat curve, so a fresh
/// install sounds bit-identical to no EQ until the user opts in. Gains are in
/// dB and go through `equalizer::normalize_gains` on read, so a hand-edited or
/// wrong-length array can't pin a bad value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EqualizerFlags {
    pub eq_enabled: bool,
    pub eq_band_gains: Vec<f32>,
    pub eq_selected_preset: String,
    /// Master gain in dB, clamped to `MIN_PREAMP_DB..=MAX_PREAMP_DB`.
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

/// `ReplayGain` (loudness normalization) preferences. Ships **off**, so a fresh
/// install plays at the raw recorded level until the user opts in; the
/// `rg_prevent_clipping` guard then defaults **on** so a boosted track can't
/// clip. `rg_mode` is a lowercase token, falling back to Album on an unknown
/// value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayGainFlags {
    pub rg_enabled: bool,
    pub rg_mode: String,
    /// Extra preamp in dB, clamped to `RG_MIN_PREAMP_DB..=RG_MAX_PREAMP_DB`.
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

/// Crossfade preferences. Ships **off**, so an install keeps the gapless
/// behaviour it already has. Once enabled, `crossfade_skip_same_album` defaults
/// **on** so continuous-mix albums stay gapless — Strawberry's and Clementine's
/// default too. `crossfade_duration_ms` is clamped to
/// `MIN_CROSSFADE_MS..=MAX_CROSSFADE_MS`.
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

/// Audio-visualizer preferences — the one feature here that ships **on**, being
/// a presentation flourish confined to the Now-Playing view rather than
/// something that alters what you hear.
///
/// `viz_enabled` decides whether the strip *mounts* and nothing more: the
/// audio-thread tap is armed by the view being on screen, so leaving this on
/// costs nothing while the view is closed.
///
/// `viz_style` is a **key**, not an index into the picker — an index would
/// silently repoint every existing install the day the style list is reordered.
/// An unrecognized key resolves back to the default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizerFlags {
    pub viz_enabled: bool,
    pub viz_style: String,
}

/// The style key a fresh install starts on, and the one an unrecognized key
/// resolves back to. `crate::ui::visualizer`'s style table must *head with the
/// same key* — its own fallbacks land on index 0 — which its tests pin.
pub const DEFAULT_VIZ_STYLE: &str = "bars";

impl Default for VisualizerFlags {
    fn default() -> Self {
        Self {
            viz_enabled: true,
            viz_style: DEFAULT_VIZ_STYLE.to_owned(),
        }
    }
}

/// Queue-behavior preferences.
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

/// Style of the custom titlebar's decoration buttons: `Standard` paints
/// Material Symbols bar icons, `Macos` the three traffic-light circles.
/// Persisted as a token so a future style needs no schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarButtonStyle {
    #[default]
    Standard,
    Macos,
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
}

impl Default for WindowFlags {
    fn default() -> Self {
        Self {
            is_maximized: false,
            always_on_top: false,
            // On Windows the DWM caption attributes paint the OS titlebar in
            // the app mantle, so the native chrome looks near-identical and
            // brings real Aero Snap and hover-peek with it. Elsewhere the
            // custom titlebar is the only chrome that carries the playback
            // controls without losing rows to a separate OS frame.
            use_native_titlebar: cfg!(target_os = "windows"),
            titlebar_button_style: TitlebarButtonStyle::Standard,
            titlebar_button_side: TitlebarButtonSide::Right,
        }
    }
}

/// System-tray toggles, both opt-in.
///
/// With `tray_enabled` off, `ui::shell::tray_bridge::install` is skipped
/// entirely — no D-Bus connection, no service thread, no action tasks — so
/// toggling it needs a restart (the `restart-tray` `Dialog` flow).
/// `close_to_tray` only hides the window when a tray icon is actually active;
/// quit is then reached through the tray menu.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayFlags {
    pub tray_enabled: bool,
    pub close_to_tray: bool,
}

/// Scrobbling toggles, all off by default. The Last.fm / `ListenBrainz`
/// *credentials* live in a separate `scrobble_credentials.json`, never here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent scrobble toggles, serde-flattened into settings.json — nesting would change the on-disk shape and break existing installs"
)]
pub struct ScrobbleFlags {
    pub lastfm_enabled: bool,
    pub listenbrainz_enabled: bool,
    /// Mirror favorites to Last.fm Loved Tracks. Independent of
    /// `lastfm_enabled` — loving isn't scrobbling — and of its sibling below.
    pub lastfm_love_enabled: bool,
    /// Mirror favorites to `ListenBrainz` recording feedback.
    pub listenbrainz_love_enabled: bool,
    /// Auto-tag scanned tracks with their `MusicBrainz` Recording ID, into both
    /// the DB and the file, so loves work on untagged libraries.
    pub mbid_auto_tag: bool,
}

/// Discord Rich Presence toggles. Nothing lives outside `settings.json` here —
/// the application id is a compile-time constant, not a credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordFlags {
    pub discord_rpc_enabled: bool,
    /// Show album artwork on the card. Its own toggle because it drives an
    /// outbound cover lookup; inert until the parent is enabled.
    pub discord_rpc_artwork: bool,
    /// Hide the card entirely while paused instead of showing a paused marker.
    pub discord_rpc_hide_when_paused: bool,
}

impl Default for DiscordFlags {
    fn default() -> Self {
        Self {
            discord_rpc_enabled: false,
            discord_rpc_artwork: true,
            discord_rpc_hide_when_paused: false,
        }
    }
}

/// The Radio section's master switch, off by default. An upgrade has to be silent
/// for an install that never asked for a network feature, which is the reason
/// `discord_rpc_enabled` ships off and what the shipped package description
/// promises of every online feature.
///
/// The live answers are the shadows on [`crate::state::AppState`] this seeds at
/// boot — every reader of all three is either on a tokio worker or in the boot
/// path, where a settings read is disk I/O for one bool.
///
/// Click reporting is the one field the derive would get wrong, so the `Default`
/// below is written by hand: it describes what the feature does rather than
/// whether it runs, and `false` there ships a directory nobody's plays are
/// counted for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RadioFlags {
    pub radio_enabled: bool,
    /// Whether to drop segmented stations from directory results.
    ///
    /// Off by default: they play. It survives its own obsolescence because a
    /// segment playlist still starts several seconds slower than a direct
    /// mount, which is a reason to skip them and not one to hide the choice.
    ///
    /// **Renamed rather than re-defaulted.** The key it replaced shipped
    /// defaulting to `true`, so every install that has ever written this file
    /// carries that value; flipping the default alone would reach nobody who
    /// already had it.
    pub radio_hide_segmented: bool,
    /// Whether playing a station tells the directory so.
    ///
    /// Opt-out rather than opt-in: the click is what popularity ordering is
    /// built from, so a user who leaves it on is paying for the ordering every
    /// other user browses by. It carries no identity beyond the request itself.
    pub radio_send_clicks: bool,
}

impl Default for RadioFlags {
    fn default() -> Self {
        Self {
            radio_enabled: false,
            radio_hide_segmented: false,
            radio_send_clicks: true,
        }
    }
}

/// Library-management toggles.
///
/// Two default-on switches, both because the off state is the surprising one.
/// `folder_watching_enabled`: every consumer player auto-watches with no toggle at
/// all, and watching off lands in a stale-UI failure mode a user can't diagnose —
/// the toggle survives as an escape valve for the inotify watch budget on huge
/// libraries. `write_ratings_to_tags`: a star that lives only in this database is
/// one a library rebuild loses and no other player can see.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "five independent settings.json keys, three of them one-shot markers; any grouping would be a container invented for the lint rather than one describing something"
)]
pub struct LibraryFlags {
    pub folder_watching_enabled: bool,
    pub music_folder_auto_added: bool,
    /// Whether the artwork store has been brought inside its size bounds once.
    ///
    /// Normalization happens at the writer, so it only ever reaches newly-scanned files —
    /// `scanner::track_is_current` guarantees an unchanged track's artwork is never re-derived.
    /// A pass over what is already there is therefore one-shot rather than continuous, and
    /// marked here rather than inferred, there being no cheap way to ask the store whether it
    /// has been swept short of reading every file in it.
    pub artwork_store_normalized: bool,
    /// Whether a star set here is also written into the file's own tag.
    ///
    /// On, the rating is portable — other players read it, and it survives a library rebuild or
    /// a move to another machine, which is the whole reason the column alone was not enough.
    /// The cost is that a one-click action rewrites the file, so it is a switch rather than an
    /// assumption.
    pub write_ratings_to_tags: bool,
    /// Whether the ratings already sitting in this library's files have been read in once.
    ///
    /// `scanner::track_is_current` skips an unchanged file outright, so a library scanned before
    /// ratings were read stays unrated no matter how many times it is rescanned. The sweep that
    /// fixes that is one-shot, and marked here rather than inferred — an unrated row is
    /// indistinguishable from one the user deliberately cleared.
    pub ratings_imported_from_tags: bool,
}

impl Default for LibraryFlags {
    fn default() -> Self {
        Self {
            folder_watching_enabled: true,
            music_folder_auto_added: false,
            artwork_store_normalized: false,
            write_ratings_to_tags: true,
            ratings_imported_from_tags: false,
        }
    }
}

/// What the diagnostics surfaces record.
///
/// `verbose_logging` is a debugging mode, and against a fixed rotation budget
/// leaving it on costs a reporter the older history. Persisted rather than
/// session-scoped so `logging::install` can start a boot at it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsFlags {
    pub verbose_logging: bool,
}

/// What the one-time Ko-fi prompt remembers. `launch_count` stops advancing
/// once `support_prompt_seen` is set, so a settled install stops rewriting
/// `settings.json` at boot rather than counting forever.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SupportFlags {
    pub launch_count: u32,
    pub support_prompt_seen: bool,
}

/// Auto-updater state persisted between launches.
///
/// `last_check_unix` and `last_manifest_etag` drive the daily-check loop's
/// elapsed gate and `If-None-Match` short-circuit; `consecutive_failures` is
/// what lengthens its re-arm against flaky-network thrash. `skipped_release` is
/// set *only* by the "Skip this version" affordance — dismissing the toast
/// deliberately doesn't, since a dismissed toast returns next launch while a
/// skipped version stays suppressed until a strictly-newer one lands.
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

/// In-app layout / visual toggles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutFlags {
    pub sidebar_collapsed: bool,
    pub progress_state_layer: bool,
    /// KDE-only: under the native titlebar, tint the sidebar and now-playing
    /// bar toward `Theme.base` while unfocused, mirroring KDE's own
    /// window-decoration fade. The field default seeds it per-desktop for a
    /// file that predates the key; `LayoutFlags::default()` stays `false` so a
    /// fresh non-KDE install boots with the row hidden.
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackdropFlags {
    pub aurora_backdrop: bool,
}

// OS / desktop-environment probes behind the defaults above. They sit in
// `services/` rather than `library/` because that is the dependency direction;
// `src/ui/appearance/` imports them from `services::settings::` directly.

/// Whether the active session is KDE Plasma. Seeds
/// `LayoutFlags.match_unfocused_to_system_bg` and hides the matching Appearance
/// row elsewhere, the behaviour it drives being KDE's own.
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

/// The host desktop's window corner radius, in pixels. Values mirror the chip
/// presets in the Appearance section, so a first launch lights up the chip
/// matching the environment.
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
            visualizer: VisualizerFlags::default(),
            queue: QueueFlags::default(),
            window: WindowFlags::default(),
            tray: TrayFlags::default(),
            scrobble: ScrobbleFlags::default(),
            discord: DiscordFlags::default(),
            radio: RadioFlags::default(),
            library: LibraryFlags::default(),
            layout: LayoutFlags::default(),
            motion: MotionFlags::default(),
            backdrop: BackdropFlags::default(),
            updates: UpdateFlags::default(),
            diagnostics: DiagnosticsFlags::default(),
            support: SupportFlags::default(),
        }
    }
}

/// Locale codes the bundled `.po` files cover, in the Language dropdown's
/// display order. Index 0 is the default and ships no catalogue — English is the
/// msgid baseline, living in the `.slint` sources directly.
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
    if lang.len() == 2 && lang.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(lang)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../tests/settings_tests.rs"]
mod tests;
