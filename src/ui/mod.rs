pub mod albums;
pub mod appearance;
pub mod artists;
pub mod artwork_cache;
pub mod backdrop;
pub mod browse;
pub mod callbacks;
pub mod chips;
pub mod detail_artwork;
pub mod detail_filter;
pub mod detail_selection;
pub mod detail_view;
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
pub mod model_diff;
pub mod model_patch;
pub mod mosaic_blur;
pub mod mosaic_hero;
pub mod my_library;
pub mod nav_history;
pub mod nav_transition;
pub mod now_playing;
pub mod now_playing_artwork;
pub mod playlists;
pub mod queue_sheet;
pub mod recently_played;
pub mod replaygain;
pub mod row_match;
pub mod search;
pub mod section_state;
pub mod settings;
pub mod settings_bind;
pub mod shell;
pub mod sleep_timer;
pub mod support;
pub mod tab_bar;
pub mod track_list_cache;
pub mod track_list_view;
pub mod track_sort;
pub mod tracks;
pub mod util;
pub mod view_ctx;
pub mod view_tag;
pub mod visualizer;
pub mod window_chrome;

// Source pins for shared components with no Rust module of their own — the
// three faked-placeholder inputs and the tooltip pill, the two pinned bands (the one
// both mosaic pages wear and My Library's own), the blur stack under both of them, the
// header row all three of those plus the Settings page share, and `IconButton`, whose
// glyph sits outside the disc and places itself — nested it costs a layer per frame of a
// band morph, centred by a layout it folds an animated press into every host's layout
// cache. `scrollbar_tests` is the odd one out: it pins a *convention* across the
// whole tree rather than one component's contract — as does `hero_blur_backdrop_tests`'
// second half, which reaches the Now Playing view because that stack is the same three
// layers written twice, `placeholder_tests`' two walks — one asking every page and every
// shell container whether it hand-rolled a tooltip frame instead of mounting the shared
// one, the other asking every shared band whether it drew a pill where it should have
// published an anchor — and `nav_transition_tests`, which asks every mount whether it wrote
// its own enter edge. That last one has a Rust module (`nav_transition`), but what it pins
// is the Slint half of the same contract, so it sits with the other tree walks.
// `startup_motion_tests` pins two components across that same seam: what the shell and the
// view mounted inside it do on the frame the window opens. `titlebar_tests` reaches a third
// tree — it holds the brand mark's theme brush to the asset it is painted over, the only
// thing here a dark-palette reviewer cannot see going wrong.
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
#[path = "tests/nav_transition_tests.rs"]
mod nav_transition_tests;
#[cfg(test)]
#[path = "tests/placeholder_tests.rs"]
mod placeholder_tests;
#[cfg(test)]
#[path = "tests/scrollbar_tests.rs"]
mod scrollbar_tests;
#[cfg(test)]
#[path = "tests/startup_motion_tests.rs"]
mod startup_motion_tests;
#[cfg(test)]
#[path = "tests/tab_search_header_tests.rs"]
mod tab_search_header_tests;
#[cfg(test)]
#[path = "tests/titlebar_tests.rs"]
mod titlebar_tests;
