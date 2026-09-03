//! Publishing what is playing to a surface outside the app, and taking commands back.
//!
//! Scrobbling, Discord Rich Presence and the OS media controls are one shape three times over,
//! and they share a crate for a reason that is not the shape: `cargo:rustc-env` reaches only the
//! crate whose build script emitted it, and the two `option_env!` API-key sites live in
//! `scrobble/providers/lastfm.rs` and `discord/mod.rs`. Splitting those two apart ships a build
//! that is silently keyless.

pub mod discord;
pub mod media_controls;
pub mod scrobble;
