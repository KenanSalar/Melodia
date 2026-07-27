// The Slint compiler's output lives in its own crate so it is built once rather
// than once per compilation of this one. Re-exported flat, so every call site
// keeps naming the generated types as `crate::AppWindow`, `crate::TrackRow`, ….
pub use melodia_ui::*;

pub mod config;
pub mod database;
pub mod entities;
pub mod error;
pub mod library;
pub mod media;
pub mod player;
pub mod services;
pub mod state;
pub mod tasks;
pub mod themes;
pub mod ui;
pub mod utils;
