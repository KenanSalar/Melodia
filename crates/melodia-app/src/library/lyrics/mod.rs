//! Lyrics for the playing track, and the one door onto them.
//!
//! Four places a sheet can come from, and a caller learns which one answered only by asking the
//! sheet itself. They resolve through one function so the switch that turns the lookup off has a
//! single place to guard, which is `library::radio`'s shape and is here for its reason.
//!
//! **`online_lookup_enabled` may not move, and `lyrics_online_enabled` is named inside it and
//! nowhere else.** The switch is enforced at the seam rather than per call site, so a fifth source
//! added later cannot reach the network around it.
//!
//! **The order is sidecar, tag, store, lookup.** A sidecar is the one a user places deliberately,
//! so it outranks whatever a tagger happened to write; the tag is the file's own and outranks what
//! a stranger uploaded; the store is only ever a copy of the last answer. That order is what makes
//! the feature correctable without a setting, since a wrong sheet is fixed by dropping a `.lrc`
//! next to the file.
//!
//! **The store sits on the local side of the switch**, which is the whole of what the switch
//! sells. Turning the lookup off buys no traffic, not no lyrics: a sheet already fetched keeps
//! being read, because reading it costs nothing anybody asked to stop paying.

mod embedded;
mod lrc;
mod online;
mod sidecar;
mod store;

use std::path::{Path, PathBuf};

use crate::state::AppState;
use melodia_core::entities::lyrics::LyricsOutcome;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;

/// What this track has, from whichever source has it.
///
/// **This owns its `spawn_blocking`**, unlike [`crate::library::tags::read_lyrics`], whose caller
/// does. The two differ because this one is a chain rather than a single read: all three local
/// sources go onto the pool together, so a track change costs one hop rather than one per source.
pub async fn for_track(state: &AppState, track: &TrackSummary) -> Result<LyricsOutcome, AppError> {
    let path = PathBuf::from(&track.file_path);
    let lyrics_dir = state.paths.lyrics_dir.clone();
    let track_path = track.file_path.clone();

    let local = state
        .runtime
        .spawn_blocking(move || read_local(&path, &lyrics_dir, &track_path))
        .await
        .map_err(AppError::io_source)??;

    if let Some(outcome) = local {
        return Ok(outcome);
    }
    if !online_lookup_enabled(state) {
        return Ok(LyricsOutcome::Absent);
    }

    online::look_up(state, track).await
}

/// The three sources that need no network, in the order a user's own file wins. Blocking.
fn read_local(
    path: &Path,
    lyrics_dir: &Path,
    track_path: &str,
) -> Result<Option<LyricsOutcome>, AppError> {
    if let Some(lyrics) = sidecar::read(path)? {
        return Ok(Some(LyricsOutcome::Sheet(lyrics)));
    }
    if let Some(lyrics) = embedded::read(path)? {
        return Ok(Some(LyricsOutcome::Sheet(lyrics)));
    }
    Ok(store::read(lyrics_dir, track_path))
}

/// Whether a track with no sheet of its own may be looked up online.
///
/// **The one reader of the setting.** A second one is a copy that can be got wrong separately,
/// which is why radio's equivalent is pinned to exactly one naming of its own flag.
///
/// Answering `false` ends the chain in [`LyricsOutcome::Absent`] rather than an error, and that is
/// the whole difference from radio's guard, which refuses. Radio unmounts its section when off, so
/// a call arriving there is a bug worth reporting; a track with no lyrics and no lookup is the
/// ordinary state of most libraries, and a panel saying so would be right.
fn online_lookup_enabled(state: &AppState) -> bool {
    state.lyrics_online_enabled.get()
}

/// Hold the on-disk store to its bounds.
///
/// Through the door like everything else here, so `tasks::lyrics_cache` names no submodule and the
/// naming scheme stays this module's business. Blocking; the task owns the `spawn_blocking`.
pub fn prune_store(paths: &melodia_core::config::Paths) -> Result<u32, AppError> {
    store::prune(&paths.lyrics_dir)
}
