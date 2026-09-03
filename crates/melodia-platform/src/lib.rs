//! Everything answering to the OS: the tray, the single-instance socket, the allocator knobs,
//! logging, crash reports, the system theme signal, desktop integration, always-on-top, the
//! Windows caption, and the updater's install-kind sliver.
//!
//! It names `melodia-core` and nothing else. That is the whole point of the crate: a feature
//! wanting a tray icon or a log file gets one without also getting the engine, the database or
//! the UI, and the two edges that used to break that — `always_on_top`'s `&AppState` and
//! `logging::install` opening `settings.json` itself — were narrowed in Phase B.

pub use melodia_core::{config, entities, error, themes, utils};

pub mod services {
    pub mod platform;
}

#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
