//! The Settings page: its chrome, and one module per card but the four facing the window, which
//! are [`crate::ui::appearance`]'s. Appearance and Window Chrome write `Theme` and the chrome
//! globals rather than a settings row, and Overflow Menu and Mini Player sit beside the window
//! toggles `window_settings` owns. The Output card is two: [`output_settings`]' rows and
//! [`signal_path`]'s live readout.
//!
//! `equalizer`, `replaygain`, `sleep_timer`, `settings_bind` and `launcher` stay at the `ui/`
//! root, each serving the page rather than belonging to it: the first two wire dialogs the Now
//! Playing overflow opens, and the rest are a player feature and two shared helpers.

pub mod about;
pub mod diagnostics;
pub mod discord_settings;
pub mod file_watching;
pub mod library_settings;
pub mod locale;
pub mod lyrics_settings;
pub mod output_settings;
pub mod playback_settings;
pub mod radio_settings;
pub mod rating_writeback;
pub mod scrobbling_settings;
pub mod settings_page;
pub mod signal_path;
pub mod updater_settings;
