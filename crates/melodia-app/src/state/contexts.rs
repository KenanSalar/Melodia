//! [`PlaybackContext`], the one dependency slice of [`AppState`], taken by `library::playback::*`.
//!
//! No second context exists: elsewhere in `library` a function takes the one field it reads or the
//! whole state, and a struct between the two would only rename one of them. The context owns its
//! `Arc`s rather than borrowing from `AppState`, so building one is six clones and it moves into
//! an `async move` without a lifetime to satisfy.

use std::sync::{Arc, OnceLock};

use melodia_core::config::Paths;
use melodia_engine::player::engine::backend::PlaybackEngine;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::{PlayerAction, PlayerState, PlayerStateHandle};
use melodia_store::database::DbPool;

use super::AppState;

/// Dependency slice consumed by `library::playback::*`.
#[derive(Clone)]
pub struct PlaybackContext {
    pub player_state: Arc<PlayerStateHandle>,
    pub sinks: Arc<PlayerSinks>,
    pub engine: Arc<PlaybackEngine>,
    pub db: DbPool,
    pub paths: Arc<Paths>,
    /// [`AppState::http_client_cell`], so a station opens on the one pool with its User-Agent.
    /// Carried rather than passed per call, resuming a paused station being a transport command
    /// like any other and `player_play` having only this.
    pub http: Arc<OnceLock<reqwest::Client>>,
}

impl PlaybackContext {
    /// [`melodia_engine::player::engine::actions::emit_and_execute`] over this context's handles,
    /// so the call sites stay one-liners through the shared execution lock.
    pub fn emit_and_execute<F>(&self, f: F)
    where
        F: FnOnce(&mut PlayerState) -> Vec<PlayerAction>,
    {
        melodia_engine::player::engine::actions::emit_and_execute(
            &*self.engine,
            &self.player_state,
            &self.sinks,
            f,
        );
    }
}

impl AppState {
    /// Snapshot a [`PlaybackContext`] from the live state.
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
