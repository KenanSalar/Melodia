//! Everything that opens a socket: the shared HTTP primitives, the four artwork and logo
//! fetchers, the radio directory client and the blocklist in front of it.
//!
//! `http_url` and `read_capped` are the two primitives every fetch shares, and the reason this
//! is a crate rather than a directory: a URL from outside the app is *parsed* rather than
//! prefix-tested, and a body is streamed under a cap rather than allocated and measured after.
//! Both are violable from any file and neither fails visibly, so the corpus walks in
//! `melodia-tidy`'s `net_primitives` hold them.

pub use melodia_core::{config, entities, error, themes, utils};

pub mod services {
    pub mod net;
}

pub mod media {
    pub use melodia_artwork::media::image;

    pub mod fetch;
}

#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
