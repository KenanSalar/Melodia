//! Settings, the two JSON state files, the artist-image orchestration, the diagnostics bundle and
//! the updater — what is left of `services/` once the three adapter groups became their own
//! crates, and what each of them has in common is naming `database` or `state` or both.

pub mod artist_images;
pub mod diagnostics;
pub mod search_history;
pub mod settings;
pub mod updater;
pub mod view_state;
