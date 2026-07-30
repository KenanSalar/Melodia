//! Settings API: read / write the on-disk settings file plus per-section
//! mutators. Split into focused sub-modules by domain:
//!
//! - [`appearance`]: theme / variant / accent, dynamic colour style,
//!   match-unfocused, corner radius.
//! - [`playback`]: gapless, play-button animation, resume on startup.
//! - [`crossfade`]: crossfade on/off, duration, manual-change + same-album
//!   exceptions, fade-on-pause.
//! - [`view`]: locale, overflow buttons, and per-view UI state — column
//!   visibility / widths, sort, browse path, nav index, detail ids,
//!   section-collapse toggles — plus the `snap_to_preset` helper.
//! - [`folders`]: library folder CRUD, watcher toggle, and `scan_folder*`.
//!
//! `settings.json` setters funnel through
//! [`crate::services::settings::mutate_settings`]; the per-view-state
//! setters in [`view`] funnel through
//! [`crate::services::view_state::mutate_view_state`] (the separate
//! `views.json` file). Each file has its own mutate lock so concurrent
//! writes to it serialize cleanly.

pub mod appearance;
pub mod crossfade;
pub mod discord;
pub mod equalizer;
pub mod folders;
pub mod playback;
pub mod replaygain;
pub mod scrobble;
pub mod updates;
pub mod view;
pub mod visualizer;

pub use appearance::{
    set_appearance, set_corner_radius, set_dynamic_color_style, set_match_unfocused_to_system_bg,
};
pub use crossfade::{
    set_crossfade_duration_ms, set_crossfade_enabled, set_crossfade_fade_on_pause,
    set_crossfade_manual, set_crossfade_skip_same_album,
};
pub use discord::{
    set_discord_rpc_artwork, set_discord_rpc_enabled, set_discord_rpc_hide_when_paused,
};
pub use equalizer::{set_eq_band_gains_and_preset, set_eq_enabled, set_eq_preamp};
pub use replaygain::{
    set_replaygain_enabled, set_replaygain_mode, set_replaygain_preamp,
    set_replaygain_prevent_clipping,
};
pub use scrobble::{
    set_scrobble_lastfm_enabled, set_scrobble_lastfm_love_enabled,
    set_scrobble_listenbrainz_enabled, set_scrobble_listenbrainz_love_enabled,
    set_scrobble_mbid_auto_tag,
};
pub use folders::{
    add_folder, get_folders, reconcile_watched_folders, remove_folder, scan_folder,
    scan_folder_internal, set_folder_watching_enabled, toggle_folder_watching,
};
pub use playback::{
    set_gapless_playback, set_play_button_animation, set_playback_speed, set_resume_on_startup,
};
pub use updates::{
    record_check_failure, record_check_success, reset_skipped_release, set_auto_check_enabled,
    set_skipped_release,
};
pub use view::{
    get_view_sort, set_artist_albums_collapsed, set_browse_path, set_favorites_artists_collapsed,
    set_favorites_most_played_collapsed, set_last_detail_id, set_last_nav_index, set_locale,
    set_overflow_button, set_settings_tab, set_view_sort, snap_to_preset, update_view_columns,
};
pub use visualizer::{set_visualizer_enabled, set_visualizer_style};

use crate::error::AppError;
use crate::services::{self, settings::SettingsData, view_state::ViewStateData};
use crate::state::AppState;

pub fn get_settings(state: &AppState) -> Result<SettingsData, AppError> {
    services::settings::read_settings(&state.paths)
}

/// Read the per-view UI state from `views.json`. Sibling of
/// [`get_settings`]; a missing / unreadable file falls back to
/// [`ViewStateData::default`].
pub fn get_view_state(state: &AppState) -> Result<ViewStateData, AppError> {
    services::view_state::read_view_state(&state.paths)
}

pub fn update_settings(
    state: &AppState,
    settings: &SettingsData,
) -> Result<(), AppError> {
    services::settings::write_settings(&state.paths, settings)
}

/// Returns "dark" or "light" based on the OS color scheme preference.
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        clippy::unused_async,
        reason = "signature mirrors the Linux variant (which awaits the XDG portal); non-Linux returns the fallback synchronously"
    )
)]
pub async fn get_system_theme() -> String {
    #[cfg(target_os = "linux")]
    {
        crate::services::system_theme::get_system_theme().await
    }
    #[cfg(not(target_os = "linux"))]
    {
        "dark".to_owned()
    }
}

// Note: `is_kde_desktop` and `get_os_corner_radius` moved to
// `services::settings` so the layer dependency (library → services)
// stays unidirectional; the services layer needs them for serde
// `default = "..."` helpers. UI code calls them via
// `services::settings::*` directly.

#[cfg(target_os = "linux")]
pub fn get_kde_colors() -> Option<crate::services::system_theme::KdeColorPalette> {
    crate::services::system_theme::get_kde_colors()
}

#[cfg(not(target_os = "linux"))]
pub fn get_kde_colors() -> Option<serde_json::Value> {
    None
}
