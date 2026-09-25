//! The Settings page: its chrome, and every section card but the four that face the window.
//!
//! Those four are [`crate::ui::appearance`]'s. Appearance and Window Chrome write `Theme`
//! brushes and the window-chrome globals rather than a settings row, and **Overflow Menu** and
//! **Mini Player** sit there only because `window_settings` already owns the window toggles
//! beside them. Otherwise it is one module per card, with one card split in two:
//! `playback_settings` wires the Output card's toggle beside its own section's, and
//! [`signal_path`] wires that card's live readout. [`settings_page`] holds the page's own tab
//! index, search predicate and responsive geometry.
//!
//! Four modules that look like they belong here stay at the `ui/` root, on what the page
//! *owns* versus what merely serves it: `equalizer` and `replaygain` wire `Dialog`
//! overlays reached from the Now Playing overflow, `sleep_timer` is a player feature
//! outright, `settings_bind` binds rows for those overlays and the visualizer as well, and
//! `launcher` is a generic open-in-the-OS capability with no caller outside Settings yet.

pub mod about;
pub mod diagnostics;
pub mod discord_settings;
pub mod file_watching;
pub mod library_settings;
pub mod locale;
pub mod lyrics_settings;
pub mod motion;
pub mod playback_settings;
pub mod radio_settings;
pub mod rating_writeback;
pub mod scrobbling_settings;
pub mod settings_page;
pub mod signal_path;
pub mod updater_settings;
