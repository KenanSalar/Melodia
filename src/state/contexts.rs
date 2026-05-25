//! Narrow dependency slices of [`AppState`]. Functions that only need a
//! subset of the global state should accept one of these instead of
//! `&AppState` so the dependency surface is explicit at the function
//! boundary — easier to read, easier to test in isolation.
//!
//! Currently exposes [`PlaybackContext`]; further per-domain contexts
//! (queue, library) are intentionally not introduced — those modules
//! touch most of `AppState` anyway and narrowing produces wide
//! impl-with-one-call structs, the over-abstraction the
//! `rust-performance` rules call out.
//!
//! `PlaybackContext` owns its `Arc`s rather than borrowing from
//! `AppState`. Each field is already `Arc`-backed under the hood, so a
//! `playback_ctx()` call costs five `Arc::clone` increments — cheap
//! enough that callers don't need to think about reuse. Owned fields
//! also dodge the "temporary doesn't live long enough" pattern when the
//! ctx is built inline in an `async move` block.

use std::sync::Arc;

use crate::config::Paths;
use crate::database::DbPool;
use crate::player::event_sink::PlayerSinks;
use crate::player::rodio_backend::RodioPlayer;
use crate::player::state::PlayerStateHandle;

use super::AppState;

/// Dependency slice consumed by `library::playback::*`. Every field is
/// `Arc`-backed; cloning the context is cheap.
#[derive(Clone)]
pub struct PlaybackContext {
    pub player_state: Arc<PlayerStateHandle>,
    pub sinks: Arc<PlayerSinks>,
    pub rodio: Arc<RodioPlayer>,
    pub db: DbPool,
    pub paths: Arc<Paths>,
}

impl AppState {
    /// Snapshot a [`PlaybackContext`] from the live state. Cheap — five
    /// `Arc::clone` calls. Build once per UI callback rather than per
    /// `library::playback::*` invocation if you're firing several in a row.
    pub fn playback_ctx(&self) -> PlaybackContext {
        PlaybackContext {
            player_state: self.player_state.clone(),
            sinks: self.sinks.clone(),
            rodio: self.rodio.clone(),
            db: self.db.clone(),
            paths: self.paths.clone(),
        }
    }
}
