//! What a decoded stream of samples can come out of, and the vocabulary the whole chain is
//! written against.
//!
//! `source::audio` imports nothing and names the interface — `AudioSource` is something the
//! output layer can pull, not something a dependency happens to accept. That is what makes a new
//! source kind a new `AudioSource` and nothing else: under one flat `player/` it could reach the
//! mixer and the state machine, and from here it cannot, because neither is in this crate's
//! manifest.
//!
//! Names the network in four files and cpal in none. `melodia-playback` is the other way round,
//! and the two dependency sets do not intersect.

pub use melodia_core::{config, entities, error, themes, utils};

pub mod player {
    pub mod source;
}

pub mod services {
    pub use melodia_net::services::net;
}

#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
