//! Settings, the two JSON state files, the artist-image orchestration, the diagnostics bundle and
//! the updater — what is left of `services/` once the three adapter groups became their own
//! crates, and what each of them has in common is naming `database` or `state` or both.
//!
//! The three that left are re-exported here so `crate::services::net` and its siblings keep
//! resolving. [`net`] is `pub(crate)`: `melodia-views` sits above this crate and reaching a socket
//! through a re-export is exactly what the split forbids.

pub use melodia_integrations::services::integrations;
pub use melodia_platform::services::platform;

pub(crate) use melodia_net::services::net;

pub mod artist_images;
pub mod diagnostics;
pub mod search_history;
pub mod settings;
pub mod updater;
pub mod view_state;
