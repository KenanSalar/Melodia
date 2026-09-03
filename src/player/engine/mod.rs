//! The state machine and what runs its decisions.
//!
//! [`state`] mutates under one lock and publishes a view model; [`actions`] is the side-effect
//! list executed after that lock drops; [`handlers`] is the monitor deciding when to advance,
//! preload or crossfade; [`backend`] is what all three drive. The pairing rule between the first
//! two is repo-wide and lives in the root `CLAUDE.md`.

pub mod actions;
pub mod backend;
pub mod event_sink;
pub mod handlers;
pub mod now_playing;
pub mod queue;
pub mod state;
pub mod types;
