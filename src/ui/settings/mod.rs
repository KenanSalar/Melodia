//! The Settings page: its chrome, and the ten section cards behind it.
//!
//! One module per card plus [`settings_page`] for the page's own tab index,
//! search predicate and responsive geometry. Everything here is reached by
//! `boot::ui_setup::install_library_settings_and_friends`, which installs them
//! in one block.
//!
//! **Two things that look like they belong here and don't.** `ui::equalizer`
//! and `ui::replaygain` wire `Dialog` overlays reached from the Now Playing
//! overflow, not this page — they are installed alongside these only for
//! convenience. And `ui::sleep_timer` is a player feature outright. All three
//! stay at the `ui/` root.
//!
//! `ui::settings_bind` and `ui::launcher` also stay at the root, and the rule
//! is what the page *owns* versus what merely serves it: `settings_bind` binds
//! a settings row for the two dialog overlays and the visualizer as well as for
//! this page, and `launcher` is a generic open-in-the-OS capability that only
//! happens to have no caller outside Settings yet.

pub mod about;
pub mod diagnostics;
pub mod discord_settings;
pub mod file_watching;
pub mod library_settings;
pub mod locale;
pub mod playback_settings;
pub mod scrobbling_settings;
pub mod settings_page;
pub mod updater_settings;
