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
//! `playback_ctx()` call costs six `Arc::clone` increments — cheap
//! enough that callers don't need to think about reuse. Owned fields
//! also dodge the "temporary doesn't live long enough" pattern when the
//! ctx is built inline in an `async move` block.

use std::sync::{Arc, OnceLock};

use crate::config::Paths;
use crate::database::DbPool;
use crate::player::backend::PlaybackEngine;
use crate::player::event_sink::PlayerSinks;
use crate::player::state::{PlayerAction, PlayerState, PlayerStateHandle};

use super::AppState;

/// Dependency slice consumed by `library::playback::*`. Every field is
/// `Arc`-backed; cloning the context is cheap.
#[derive(Clone)]
pub struct PlaybackContext {
    pub player_state: Arc<PlayerStateHandle>,
    pub sinks: Arc<PlayerSinks>,
    pub engine: Arc<PlaybackEngine>,
    pub db: DbPool,
    pub paths: Arc<Paths>,
    /// The same lazily-built client `AppState`, scrobbling and Discord presence share, so a
    /// station opens on the one connection pool and carries the `Melodia/<version>` User-Agent
    /// with it. Carried here rather than passed per call because resuming a paused station is a
    /// transport command like any other, and `player_play` has only this.
    pub http: Arc<OnceLock<reqwest::Client>>,
}

impl PlaybackContext {
    /// Serialized mutate → emit → execute against this context's handles. Thin
    /// forwarder over [`crate::player::actions::emit_and_execute`] so the
    /// `library::playback::*` / `library::queue::*` call sites stay one-liners
    /// while still routing through the shared execution lock.
    pub fn emit_and_execute<F>(&self, f: F)
    where
        F: FnOnce(&mut PlayerState) -> Vec<PlayerAction>,
    {
        crate::player::actions::emit_and_execute(&*self.engine, &self.player_state, &self.sinks, f);
    }
}

impl AppState {
    /// Snapshot a [`PlaybackContext`] from the live state. Cheap — six
    /// `Arc::clone` calls. Build once per UI callback rather than per
    /// `library::playback::*` invocation if you're firing several in a row.
    pub fn playback_ctx(&self) -> PlaybackContext {
        PlaybackContext {
            player_state: self.player_state.clone(),
            sinks: self.sinks.clone(),
            engine: self.engine.clone(),
            db: self.db.clone(),
            paths: self.paths.clone(),
            http: self.http_client_cell(),
        }
    }
}
