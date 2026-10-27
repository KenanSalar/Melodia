//! Background tasks, each module's `spawn` taking a [`TaskSpawner`] and whatever slices of
//! `AppState` it needs.

pub mod artwork_renormalize;
pub mod artwork_restore;
pub mod artwork_sweep;
pub mod audio_health;
pub mod device_volume;
pub mod discord_presence;
pub mod file_event_processor;
pub mod heap_trim;
pub mod lyrics_cache;
pub mod material_you;
pub mod one_shot;
pub mod play_count_flusher;
pub mod playback_monitor;
pub mod queue_prune;
pub mod queue_window;
pub mod radio_logo_cache;
pub mod rating_import;
pub mod rating_writeback;
pub mod resume_watching;
pub mod retroactive_hash;
pub mod rss_sampler;
pub mod scrobble;
pub mod search_text_backfill;
pub mod spawner;
pub mod tag_backfill;
pub mod updater_daily;

pub use spawner::TaskSpawner;
