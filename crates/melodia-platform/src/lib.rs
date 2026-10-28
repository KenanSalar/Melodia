//! Everything answering to the OS: the tray, the single-instance socket, the allocator knobs,
//! logging, crash reports, the system theme signal, desktop integration, always-on-top, the
//! Windows caption, and the updater's install-kind sliver.
//!
//! It names `melodia-core` and nothing else, so a feature wanting a tray icon or a log file gets
//! one without the engine, the database or the UI.

pub mod services {
    pub mod platform;
}
