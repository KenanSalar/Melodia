//! The library as it sits on disk: the `SQLite` schema and its queries, the scanner that fills
//! them, the filesystem watcher that keeps them true, and the tag reader and writer under both.
//!
//! Above `melodia-artwork`, which it stores covers through, and above `melodia-audio`, which it
//! asks for a duration the container won't state. Below everything that decides *what* to scan:
//! nothing here names a setting, a task or the app's state.

pub use melodia_core::{config, entities, error, themes, utils};

pub mod database;

pub mod media {
    pub use melodia_artwork::media::image;

    pub mod ingest;
}

pub mod player {
    pub use melodia_audio::player::source;
}

#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
