use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rodio::mixer::Mixer;
use rodio::{Decoder, Player};

use crate::config::Paths;
use crate::error::AppError;

use super::types::PersistableQueue;

/// Trait abstracting audio playback operations.
/// Implemented by `RodioPlayer` for production and mock backends for testing.
pub trait PlayerBackend: Send + Sync {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
    ) -> Result<(), AppError>;
    fn resume(&self);
    fn pause(&self);
    fn stop(&self);
    fn seek(&self, position_ms: u64);
    fn set_volume(&self, volume: f64);
    fn set_speed(&self, speed: f64);
    fn preload_gapless(&self, file_path: Option<&str>);
}

/// Blanket impl: any `Deref<Target = T>` where T: `PlayerBackend` also implements `PlayerBackend`.
/// This covers `Arc<RodioPlayer>`, `State<'_, Arc<RodioPlayer>>`, etc.
impl<T: std::ops::Deref + Send + Sync> PlayerBackend for T
where
    T::Target: PlayerBackend,
{
    fn play_media(&self, file_path: &str, volume: f64, speed: f64, start_position_ms: Option<u64>) -> Result<(), AppError> {
        (**self).play_media(file_path, volume, speed, start_position_ms)
    }
    fn resume(&self) { (**self).resume(); }
    fn pause(&self) { (**self).pause(); }
    fn stop(&self) { (**self).stop(); }
    fn seek(&self, position_ms: u64) { (**self).seek(position_ms); }
    fn set_volume(&self, volume: f64) { (**self).set_volume(volume); }
    fn set_speed(&self, speed: f64) { (**self).set_speed(speed); }
    fn preload_gapless(&self, file_path: Option<&str>) { (**self).preload_gapless(file_path); }
}

impl PlayerBackend for RodioPlayer {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
    ) -> Result<(), AppError> {
        self.play_media(file_path, volume, speed, start_position_ms)
    }
    fn resume(&self) { self.resume(); }
    fn pause(&self) { self.pause(); }
    fn stop(&self) { self.stop(); }
    fn seek(&self, position_ms: u64) { self.seek(position_ms); }
    fn set_volume(&self, volume: f64) { self.set_volume(volume); }
    fn set_speed(&self, speed: f64) { self.set_speed(speed); }
    fn preload_gapless(&self, file_path: Option<&str>) { self.preload_gapless(file_path); }
}

/// Result of checking the Rodio player queue in a single lock acquisition.
#[derive(Debug, PartialEq)]
pub enum PlaybackCheck {
    /// Gapless transition happened (queue depth dropped from 2 to 1).
    GaplessTransition,
    /// All sources drained — end of stream.
    EndOfStream,
    /// Still playing normally.
    Playing,
}

/// Pure decision logic for playback state detection.
/// Separated from `RodioPlayer::check_playback_state` for testability.
pub fn evaluate_playback_check(was_gapless: bool, queue_len: usize, is_empty: bool) -> PlaybackCheck {
    if was_gapless && queue_len <= 1 {
        return PlaybackCheck::GaplessTransition;
    }
    if is_empty {
        return PlaybackCheck::EndOfStream;
    }
    PlaybackCheck::Playing
}

/// Pure speed-adjusted position calculation, separated for testability.
///
/// `speed` is in `0.25..=4.0`. Real-world track durations stay well below
/// `2^53` ms (~285 years), so the u64 ↔ f64 round-trip is lossless and the
/// final f64 → u64 truncation can never produce a nonsensical value.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "speed * ms math intentionally uses f64; result is non-negative, finite, and fits in u64 for any real audio duration"
)]
pub fn compute_position(wall_time: Duration, speed: f64) -> u64 {
    let ms = u64::try_from(wall_time.as_millis()).unwrap_or(u64::MAX);
    (ms as f64 * speed) as u64
}

pub struct RodioPlayer {
    // Mutex groups multi-op sequences (clear → set_volume → append → play → seek)
    // atomically; rodio's Player is already Send+Sync on its own.
    player: std::sync::Mutex<Player>,
    gapless_pending: AtomicBool,
}

impl RodioPlayer {
    pub fn new(mixer: &Mixer) -> Self {
        let player = Player::connect_new(mixer);
        player.pause();
        Self {
            player: std::sync::Mutex::new(player),
            gapless_pending: AtomicBool::new(false),
        }
    }

    /// Start playback of a file, optionally seeking to a position — all under a single lock.
    /// This prevents the playback monitor from observing position ~0 between play and seek.
    pub fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
    ) -> Result<(), AppError> {
        let player = self.lock_player();
        player.clear();
        player.set_volume(narrow_audio_param(volume));
        player.set_speed(narrow_audio_param(speed));
        let source = decode_file(file_path)?;
        player.append(source);
        player.play();
        self.gapless_pending.store(false, Ordering::Release);
        if let Some(pos) = start_position_ms {
            log::debug!("Resuming playback at {pos}ms");
            if let Err(e) = player.try_seek(Duration::from_millis(pos)) {
                log::warn!("Seek after play failed: {e}");
            }
        }
        Ok(())
    }

    pub fn resume(&self) {
        let player = self.lock_player();
        player.play();
    }

    pub fn pause(&self) {
        let player = self.lock_player();
        player.pause();
    }

    /// Stop playback. `Player::clear()` removes all sources and pauses automatically.
    pub fn stop(&self) {
        let player = self.lock_player();
        player.clear();
        self.gapless_pending.store(false, Ordering::Release);
    }

    pub fn seek(&self, position_ms: u64) {
        let player = self.lock_player();
        if let Err(e) = player.try_seek(Duration::from_millis(position_ms)) {
            log::warn!("Seek failed: {e}");
        }
    }

    pub fn set_volume(&self, volume: f64) {
        let player = self.lock_player();
        player.set_volume(narrow_audio_param(volume));
    }

    pub fn set_speed(&self, speed: f64) {
        let player = self.lock_player();
        player.set_speed(narrow_audio_param(speed));
    }

    /// Whether a gapless source is currently staged behind the playing one.
    /// Used by the playback monitor to avoid re-issuing the late preload each tick.
    pub fn is_gapless_preloaded(&self) -> bool {
        self.gapless_pending.load(Ordering::Acquire)
    }

    pub fn preload_gapless(&self, file_path: Option<&str>) {
        match file_path {
            Some(path) => {
                let player = self.lock_player();
                match decode_file(path) {
                    Ok(source) => {
                        player.append(source);
                        self.gapless_pending.store(true, Ordering::Release);
                    }
                    Err(e) => {
                        log::warn!("Failed to preload gapless track {path}: {e}");
                        self.gapless_pending.store(false, Ordering::Release);
                    }
                }
            }
            None => {
                self.gapless_pending.store(false, Ordering::Release);
            }
        }
    }

    /// Query the current playback position in milliseconds.
    /// `get_pos()` returns wall-clock time — multiply by speed for actual media position.
    pub fn query_position(&self) -> u64 {
        let player = self.lock_player();
        let wall_time = player.get_pos();
        let speed = f64::from(player.speed());
        compute_position(wall_time, speed)
    }

    /// Check playback state in a single lock acquisition to avoid TOCTOU races
    /// between gapless transition detection and end-of-stream detection.
    pub fn check_playback_state(&self) -> PlaybackCheck {
        let was_gapless = self.gapless_pending.load(Ordering::Acquire);
        let player = self.lock_player();
        let queue_len = player.len();
        let is_empty = player.empty();
        drop(player);

        let result = evaluate_playback_check(was_gapless, queue_len, is_empty);
        if result == PlaybackCheck::GaplessTransition {
            self.gapless_pending.store(false, Ordering::Release);
        }
        result
    }

    /// Lock the Player mutex, recovering from poison rather than panicking.
    fn lock_player(&self) -> std::sync::MutexGuard<'_, Player> {
        self.player.lock().unwrap_or_else(|poisoned| {
            log::error!("RodioPlayer mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }
}

fn decode_file(path: &str) -> Result<Decoder<BufReader<File>>, AppError> {
    let file =
        File::open(path).map_err(|e| AppError::Player(format!("Cannot open {path}: {e}")))?;
    let file_len = file.metadata().map(|m| m.len()).ok();

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // 64 KB BufReader (8× the std default of 8 KB) cuts read syscalls during
    // decode by the same factor. Symphonia pulls frames in chunks well above
    // 8 KB for most formats, so the small buffer was triggering extra refills
    // per frame. 64 KB matches typical FLAC/MP3 frame-cluster sizes without
    // bloating per-track memory meaningfully.
    let mut builder = Decoder::builder()
        .with_data(BufReader::with_capacity(64 * 1024, file))
        .with_hint(ext)
        .with_gapless(true)
        .with_seekable(true);

    if let Some(len) = file_len {
        builder = builder.with_byte_len(len);
    }

    builder
        .build()
        .map_err(|e| AppError::Player(format!("Decode error for {path}: {e}")))
}

/// Load persisted queue from disk (synchronous — safe for use in startup).
pub fn load_queue_from_disk_sync(paths: &Paths) -> Option<PersistableQueue> {
    if !paths.queue_path.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&paths.queue_path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Narrow a backend-side `f64` audio parameter (volume 0.0..=1.0, speed
/// 0.25..=4.0) to the `f32` Rodio's API expects.
#[allow(
    clippy::cast_possible_truncation,
    reason = "audio params are bounded constants whose round-trip through f32 is below the perceptual threshold"
)]
fn narrow_audio_param(v: f64) -> f32 {
    v as f32
}

#[cfg(test)]
#[path = "tests/rodio_backend_tests.rs"]
mod tests;
