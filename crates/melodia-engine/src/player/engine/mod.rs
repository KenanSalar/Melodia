//! The state machine and what runs its decisions: [`state`] mutates and publishes, [`actions`]
//! runs the side effects once its lock drops, [`handlers`] is the monitor, and [`backend`] is what
//! all three drive.

pub mod actions;
pub mod backend;
pub mod event_sink;
#[doc(hidden)]
pub mod fixtures;
pub mod handlers;
pub mod now_playing;
pub mod queue;
pub mod queue_window;
pub mod signal_path;
pub mod state;
pub mod types;
