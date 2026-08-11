//! Long-running background tasks spawned at startup.
//!
//! Each task module exposes a `pub fn spawn(spawner: &TaskSpawner, …)` that
//! takes the unified [`spawner::TaskSpawner`] (`task_tracker` + shutdown token
//! bundle) plus whatever extra slices it needs from `AppState`. Most loops
//! live in this directory directly; a handful still delegate into
//! `player/` for state-machine-coupled work.

pub mod audio_health;
pub mod discord_presence;
pub mod file_event_processor;
pub mod first_launch;
pub mod heap_trim;
pub mod material_you;
pub mod mbid_backfill;
pub mod play_count_flusher;
pub mod playback_monitor;
pub mod queue_prune;
pub mod retroactive_hash;
pub mod rss_sampler;
pub mod scrobble;
pub mod spawner;
pub mod updater_daily;

pub use spawner::TaskSpawner;
