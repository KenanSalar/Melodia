pub mod albums;
pub mod appearance;
pub mod artists;
pub mod artwork_cache;
pub mod aurora;
pub mod backdrop;
pub mod browse;
pub mod callbacks;
pub mod card_grid;
pub mod clipboard;
pub mod cover_generation;
pub mod detail_artwork;
pub mod detail_view;
pub mod drag;
pub mod equalizer;
pub mod favorites;
pub mod file_dialog;
pub mod genres;
pub mod grid_prewarm;
pub mod grid_rows;
pub mod hero_backdrop;
pub mod hero_chips;
pub mod hero_folds;
pub mod launcher;
pub mod list_selection;
pub mod locale_refresh;
pub mod model_diff;
pub mod model_patch;
pub mod mosaic_hero;
pub mod most_played;
pub mod my_library;
pub mod name_palette;
pub mod nav_history;
pub mod nav_transition;
pub mod now_playing;
pub mod now_playing_artwork;
pub mod onboarding;
pub mod paged_rows;
pub mod playlists;
pub mod queue_panel;
pub mod radio;
pub mod recently_played;
pub mod replaygain;
pub mod row_match;
pub mod search;
pub mod section_state;
pub mod settings;
pub mod settings_bind;
pub mod shell;
pub mod signal;
pub mod sleep_timer;
pub mod support;
pub mod tab_bar;
pub mod track_columns;
pub mod track_detail;
pub mod track_links;
pub mod track_list_view;
pub mod track_sort;
pub mod tracks;
pub mod util;
pub mod view_ctx;
pub mod view_tag;
pub mod visualizer;
pub mod window_chrome;

#[cfg(test)]
pub(crate) mod fixtures;

// Source pins for shared components with no Rust module of their own, each reading the one
// component it is about by name; a walk over the whole tree is `crates/melodia/tests/`'s.
#[cfg(test)]
#[path = "tests/aurora_backdrop_tests.rs"]
mod aurora_backdrop_tests;
#[cfg(test)]
#[path = "tests/caption_tests.rs"]
mod caption_tests;
#[cfg(test)]
#[path = "tests/detail_restore_tests.rs"]
mod detail_restore_tests;
#[cfg(test)]
#[path = "tests/entity_card_tests.rs"]
mod entity_card_tests;
#[cfg(test)]
#[path = "tests/frameless_tests.rs"]
mod frameless_tests;
#[cfg(test)]
#[path = "tests/hero_blur_backdrop_tests.rs"]
mod hero_blur_backdrop_tests;
#[cfg(test)]
#[path = "tests/icon_button_tests.rs"]
mod icon_button_tests;
#[cfg(test)]
#[path = "tests/library_tab_band_tests.rs"]
mod library_tab_band_tests;
#[cfg(test)]
#[path = "tests/mosaic_tab_hero_tests.rs"]
mod mosaic_tab_hero_tests;
#[cfg(test)]
#[path = "tests/mount_fade_tests.rs"]
mod mount_fade_tests;
#[cfg(test)]
#[path = "tests/placeholder_tests.rs"]
mod placeholder_tests;
#[cfg(test)]
#[path = "tests/play_count_badge_tests.rs"]
mod play_count_badge_tests;
#[cfg(test)]
#[path = "tests/slider_pointer_tests.rs"]
mod slider_pointer_tests;
#[cfg(test)]
#[path = "tests/startup_motion_tests.rs"]
mod startup_motion_tests;
#[cfg(test)]
#[path = "tests/tab_search_header_tests.rs"]
mod tab_search_header_tests;
#[cfg(test)]
#[path = "tests/titlebar_tests.rs"]
mod titlebar_tests;
