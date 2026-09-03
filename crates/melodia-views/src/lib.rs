//! The Slint bridge: twenty view slices, the shared component library under them, and the
//! callbacks that wire the two together.
//!
//! It sits above every other crate and below nothing, which is what lets the exclusion be a
//! manifest rather than a convention: **no `melodia-store`, no `melodia-net`**. The UI reaches
//! the database through `melodia-app`'s library API, and it opens no socket at all. `melodia-app`
//! keeps `database`, `media::{ingest,fetch}`, `services::net` and `player::source` `pub(crate)`
//! for the same reason, so neither door is open.
//!
//! Not split further, and the argument is in `docs/plans/WORKSPACE_SPLIT.md`: the slices are a
//! dense mesh, the component library imports fourteen of them, and cutting it needs a view
//! registry nothing else in the tree wants.

// The Slint compiler's output. Flat, so every call site keeps naming the generated types as
// `crate::AppWindow`, `crate::TrackRow`, … — roughly seventy of them arrive this way.
pub use melodia_ui::*;

pub use melodia_core::{config, entities, error, themes, utils};

pub use melodia_app::{library, state, tasks};

pub mod media {
    pub use melodia_artwork::media::image;
}

pub mod player {
    pub use melodia_engine::player::engine;
    pub use melodia_playback::player::playback;
}

pub mod services {
    pub use melodia_app::services::{diagnostics, settings, updater, view_state};
    pub use melodia_integrations::services::integrations;
    pub use melodia_platform::services::platform;
}

pub mod ui;

// The corpus walkers and env-lock fixtures. Aliased so `crate::test_support::…` keeps resolving
// in the test modules that spell it.
#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
