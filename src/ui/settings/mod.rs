//! The Settings page: its chrome, and ten of the thirteen section cards.
//!
//! The other three are [`crate::ui::appearance`]'s — Appearance and Window Chrome because
//! they write `Theme` brushes and the window-chrome globals rather than a settings row,
//! **Overflow Menu** only because `window_settings` already owns the window toggles beside
//! it. One module per card, plus [`settings_page`] for the page's own tab index, search
//! predicate and responsive geometry.
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
pub mod motion;
pub mod playback_settings;
pub mod scrobbling_settings;
pub mod settings_page;
pub mod updater_settings;
