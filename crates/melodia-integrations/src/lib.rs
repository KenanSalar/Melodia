//! The three surfaces outside the app that get told what is playing: scrobbling, Discord
//! presence and the OS media controls.
//!
//! One crate because `cargo:rustc-env` reaches only the crate whose build script emitted it, and
//! the two `option_env!` key reads sit in `scrobble::providers::lastfm` and `discord`: split across
//! net and app, a build ships keyless with nothing anywhere to say so. `media_controls` is here
//! for the shape rather than the keys, publishing now-playing state outward and taking commands
//! back as `discord` does; the alternative put the engine, and cpal with it, under every crate
//! wanting a tray icon.

pub mod services {
    pub mod integrations;
}
