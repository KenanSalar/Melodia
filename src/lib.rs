// The Slint compiler's output lives in its own crate so it is built once rather
// than once per compilation of this one. Re-exported flat, so every call site
// keeps naming the generated types as `crate::AppWindow`, `crate::TrackRow`, ….
pub use melodia_ui::*;

// What `melodia-core` took, re-exported so `crate::error::AppError` and its siblings keep
// resolving from the modules still here. An explicit list rather than a glob: a facade is how a
// dependent reaches past a manifest it was meant to be stopped by.
pub use melodia_core::{config, entities, error, themes, utils};

pub mod database;
pub mod library;
pub mod media;
pub mod player;
pub mod services;
pub mod state;
pub mod tasks;
// The corpus walkers and env-lock fixtures, in their own crate so every member can dev-depend
// on them. Aliased rather than re-imported per file: `crate::test_support::…` is what the ~60
// test modules spelling it already say.
#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
pub mod ui;
