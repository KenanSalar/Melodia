pub mod allocator;
pub mod always_on_top;
pub mod artist_images;
pub mod crash_report;
#[cfg(target_os = "linux")]
pub mod desktop_integration;
pub mod diagnostics;
pub mod discord;
#[cfg(target_os = "windows")]
pub mod dwm_titlebar;
pub mod logging;
pub mod material_you;
pub mod media_controls;
pub mod net;
pub mod radio_blocklist;
pub mod radio_browser;
pub mod scrobble;
pub mod search_history;
pub mod settings;
pub mod single_instance;
#[cfg(target_os = "linux")]
pub mod system_theme;
pub mod tray;
pub mod updater;
pub mod view_state;

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
