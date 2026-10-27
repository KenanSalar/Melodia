//! Adapters over what the OS provides: the tray, the window server, the desktop database, the
//! allocator's glibc knobs, the log sink and the crash hook.
//!
//! [`install_kind`] is here on evidence rather than by category: `crash_report` stamps its target
//! key into a report and `desktop_integration` bakes its path into an `Exec=` line, and a second
//! consumer is what separates a platform primitive from a feature's internals.

pub mod allocator;
pub mod always_on_top;
#[cfg(target_os = "windows")]
pub mod color_mode;
pub mod crash_report;
pub mod desktop;
#[cfg(target_os = "linux")]
pub mod desktop_integration;
#[cfg(target_os = "windows")]
pub mod dwm_titlebar;
pub mod install_kind;
pub mod logging;
#[cfg(target_os = "windows")]
mod registry;
pub mod single_instance;
#[cfg(target_os = "linux")]
pub mod system_theme;
pub mod tray;
#[cfg(target_os = "windows")]
pub mod window_border;
