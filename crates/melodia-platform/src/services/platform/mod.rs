//! What the app asks of the OS it happens to be running on.
//!
//! Everything here is an adapter over a platform capability: the tray, the media-key surface's
//! neighbours, the window server, the desktop database, the allocator's glibc knobs, the log sink
//! and the crash hook. It answers only to `core`, which is what lets every layer above lean on it.
//!
//! [`install_kind`] is the odd member and is here on evidence rather than by category. It answers
//! *which* install this is and where the replaceable binary lives, and it is not the updater's
//! private business: `crash_report` stamps the target key into a report and
//! `desktop_integration` bakes the path into a `.desktop` `Exec=` line, both of them here. A
//! second consumer is what separates a platform primitive from a feature's internals.

pub mod allocator;
pub mod always_on_top;
pub mod crash_report;
#[cfg(target_os = "linux")]
pub mod desktop_integration;
#[cfg(target_os = "windows")]
pub mod dwm_titlebar;
pub mod install_kind;
pub mod logging;
pub mod single_instance;
#[cfg(target_os = "linux")]
pub mod system_theme;
pub mod tray;
