//! Lyrics for the playing track, and the one door onto them.
//!
//! Three places a sheet can come from, and a caller learns which one answered only by asking the
//! sheet itself. They resolve through one function so the switch that turns the lookup off has a
//! single place to guard, which is `library::radio`'s shape and is here for its reason.
//!
//! **`online_lookup_enabled` may not move, and `lyrics_online_enabled` is named inside it and
//! nowhere else.** The switch is enforced at the seam rather than per call site, so a fourth
//! source added later cannot reach the network around it.
//!
//! **The order is sidecar, then tag, then lookup.** A sidecar is the one of the three a user
//! places deliberately, so it outranks whatever a tagger happened to write and whatever a stranger
//! uploaded. That is also what makes the feature correctable without a setting: a wrong sheet is
//! fixed by dropping a `.lrc` next to the file.

mod embedded;
mod lrc;
mod online;
mod sidecar;

use std::path::{Path, PathBuf};

use crate::state::AppState;
use melodia_core::entities::lyrics::Lyrics;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;

/// The sheet for a track, or `None` where there is none to be had.
///
/// **This owns its `spawn_blocking`**, unlike [`crate::library::tags::read_lyrics`], whose caller
/// does. The two differ because this one is a chain rather than a single read: both file sources
/// go onto the pool together, so a track change costs one hop rather than one per source.
pub async fn for_track(state: &AppState, track: &TrackSummary) -> Result<Option<Lyrics>, AppError> {
    let path = PathBuf::from(&track.file_path);
    let from_file = state
        .runtime
        .spawn_blocking(move || read_file_sources(&path))
        .await
        .map_err(AppError::io_source)??;

    if from_file.is_some() {
        return Ok(from_file);
    }
    if !online_lookup_enabled(state) {
        return Ok(None);
    }

    online::look_up(state, track).await
}

/// The two sources that need no network, in the order a user's own file wins. Blocking.
fn read_file_sources(path: &Path) -> Result<Option<Lyrics>, AppError> {
    if let Some(lyrics) = sidecar::read(path)? {
        return Ok(Some(lyrics));
    }
    embedded::read(path)
}

/// Whether a track with no sheet of its own may be looked up online.
///
/// **The one reader of the setting.** A second one is a copy that can be got wrong separately,
/// which is why radio's equivalent is pinned to exactly one naming of its own flag.
///
/// Answering `false` means the chain ends in `None` rather than an error, and that is the whole
/// difference from radio's guard, which refuses. Radio unmounts its section when off, so a call
/// arriving there is a bug worth reporting; a track with no lyrics and no lookup is the ordinary
/// state of most libraries, and a panel saying so would be right.
fn online_lookup_enabled(state: &AppState) -> bool {
    state.lyrics_online_enabled.get()
}
