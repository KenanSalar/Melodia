pub mod about;
pub mod albums;
pub mod appearance;
pub mod artists;
pub mod backdrop;
pub mod bridge;
pub mod browse;
pub mod callbacks;
pub mod detail_artwork;
pub mod detail_filter;
pub mod detail_selection;
pub mod detail_view;
pub mod discord_settings;
pub mod equalizer;
pub mod replaygain;
pub mod visualizer;
pub mod event_sink;
pub mod favorites;
pub mod file_watching;
pub mod genres;
pub mod grid_prewarm;
pub mod hero_backdrop;
pub mod library_settings;
pub mod list_selection;
pub mod locale;
pub mod mini_player;
pub mod model_diff;
pub mod model_patch;
pub mod mosaic_blur;
pub mod nav_history;
pub mod nav_transition;
pub mod notifications;
pub mod now_playing;
pub mod now_playing_artwork;
pub mod playback_settings;
pub mod playlists;
pub mod queue_sheet;
pub mod recently_played;
pub mod scrobbling_settings;
pub mod search;
pub mod section_state;
pub mod settings_bind;
pub mod settings_page;
pub mod sleep_timer;
pub mod tab_bar;
pub mod track_list_view;
pub mod track_sort;
pub mod tracks;
pub mod tray_bridge;
pub mod updater_settings;
pub mod util;
pub mod window_chrome;

// Source pins for shared components with no Rust module of their own — the
// three faked-placeholder inputs and the tooltip pill.
#[cfg(test)]
#[path = "tests/placeholder_tests.rs"]
mod placeholder_tests;
