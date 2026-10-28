//! The settings API, one submodule per section. A setter only commits, its UI callback having
//! applied the value first, any `AppState` shadow included; one that does more says so.
//!
//! `settings.json` writes funnel through [`crate::services::settings::mutate_settings`] and
//! `views.json` writes through [`crate::services::view_state::mutate_view_state`], each with its
//! own lock, so a burst of clicks can't race the read-mutate-write window.

pub mod appearance;
pub mod crossfade;
pub mod diagnostics;
pub mod discord;
pub mod equalizer;
pub mod folders;
pub mod lyrics;
pub mod onboarding;
pub mod playback;
pub mod radio;
pub mod ratings;
pub mod replaygain;
pub mod scrobble;
pub mod support;
pub mod updates;
pub mod view;
pub mod visualizer;

pub use appearance::{
    seed_theme_preference, set_appearance, set_corner_radius, set_dynamic_color_style,
    set_match_unfocused_to_system_bg,
};
pub use crossfade::{
    set_crossfade_duration_ms, set_crossfade_enabled, set_crossfade_fade_on_pause,
    set_crossfade_manual, set_crossfade_skip_same_album,
};
pub use diagnostics::set_verbose_logging;
pub use discord::{
    set_discord_rpc_artwork, set_discord_rpc_enabled, set_discord_rpc_hide_when_paused,
};
pub use equalizer::{set_eq_band_gains_and_preset, set_eq_enabled, set_eq_preamp};
pub use folders::{
    add_folder, get_folders, remove_folder, set_folder_watching_enabled, suggested_music_folder,
    toggle_folder_watching,
};
pub use lyrics::{set_lyrics_enabled, set_lyrics_online_enabled, set_lyrics_romanization_shown};
pub use onboarding::set_onboarding_seen;
pub use playback::{
    reset_for_bit_perfect, set_gapless_playback, set_output_choice, set_output_follow_rate,
    set_output_paused_device, set_output_resampler, set_output_resync_ms,
    set_play_button_animation, set_playback_speed, set_resume_on_startup, switch_to_bit_perfect,
};
pub use radio::{
    set_radio_enabled, set_radio_hide_segmented, set_radio_scrobble, set_radio_send_clicks,
};
pub use ratings::set_write_ratings_to_tags;
pub use replaygain::{
    set_replaygain_enabled, set_replaygain_mode, set_replaygain_preamp,
    set_replaygain_prevent_clipping,
};
pub use scrobble::{
    set_scrobble_lastfm_enabled, set_scrobble_lastfm_love_enabled,
    set_scrobble_listenbrainz_enabled, set_scrobble_listenbrainz_love_enabled,
};
pub use support::{mark_support_prompt_seen, record_launch};
pub use updates::{set_auto_check_enabled, set_skipped_release};
pub use view::{
    get_view_sort, set_artist_albums_collapsed, set_browse_path, set_browse_view_mode,
    set_favorites_tab, set_last_detail_id, set_last_nav_index, set_locale, set_my_library_tab,
    set_overflow_button, set_radio_tab, set_recently_played_tab, set_settings_category,
    set_view_sort, snap_to_preset, update_view_columns, update_view_marks,
};
pub use visualizer::{set_visualizer_enabled, set_visualizer_style};

use crate::services::{self, settings::SettingsData, view_state::ViewStateData};
use melodia_core::config::Paths;
use melodia_core::error::AppError;

pub fn get_settings(paths: &Paths) -> Result<SettingsData, AppError> {
    services::settings::read_settings(paths)
}

/// A missing or unreadable `views.json` falls back to [`ViewStateData::default`].
pub fn get_view_state(paths: &Paths) -> Result<ViewStateData, AppError> {
    services::view_state::read_view_state(paths)
}
