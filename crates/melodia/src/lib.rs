// The Slint compiler's output lives in its own crate so it is built once rather
// than once per compilation of this one. Re-exported flat, so every call site
// keeps naming the generated types as `crate::AppWindow`, `crate::TrackRow`, ….
pub use melodia_ui::*;

// What `melodia-core` took, re-exported so `crate::error::AppError` and its siblings keep
// resolving from the modules still here. An explicit list rather than a glob: a facade is how a
// dependent reaches past a manifest it was meant to be stopped by.
pub use melodia_core::{config, entities, error, themes, utils};

pub use melodia_app::{library, state, tasks};
pub use melodia_store::database;
// `src/ui/` is `melodia-views` now, re-exported under the name `main.rs`, `boot/` and `tests/`
// already spell. The crate is aliased at its `ui` module rather than at its root so those call
// sites read `ui::callbacks::wire_all` unchanged — which `boot`'s own source-text pins count.
pub use melodia_views::ui;

pub mod media;
pub mod player;
pub mod services;
