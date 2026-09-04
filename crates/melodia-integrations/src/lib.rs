//! The three surfaces outside the app that get told what is playing: scrobbling, Discord
//! presence and the OS media controls.
//!
//! One crate rather than three placements, and the reason is the build script beside this file.
//! `cargo:rustc-env` reaches only the crate whose script emitted it, and the two `option_env!`
//! key reads sit in `scrobble::providers::lastfm` and `discord`, so splitting them across net and
//! app ships a keyless build with nothing anywhere to say so.
//!
//! `media_controls` is here for the shape rather than for the keys: publish now-playing state to
//! a surface outside the app and take transport commands back is exactly what `discord` does, and
//! the alternative put the engine, and cpal with it, under every crate wanting a tray icon.

pub mod services {
    pub mod integrations;
}
