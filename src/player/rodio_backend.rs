use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rodio::mixer::Mixer;
use rodio::{Decoder, Player};

use crate::config::Paths;
use crate::error::AppError;

use super::equalizer::{self, EqShared, EqSource};
use super::replaygain::{ReplayGainShared, RgMode, TrackReplayGain};
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
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError>;
    fn resume(&self);
    fn pause(&self);
    fn stop(&self);
    fn seek(&self, position_ms: u64);
    fn set_volume(&self, volume: f64);
    fn set_speed(&self, speed: f64);
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain);
}

/// Blanket impl: any `Deref<Target = T>` where T: `PlayerBackend` also implements `PlayerBackend`.
/// This covers `Arc<RodioPlayer>`, `State<'_, Arc<RodioPlayer>>`, etc.
impl<T: std::ops::Deref + Send + Sync> PlayerBackend for T
where
    T::Target: PlayerBackend,
{
    fn play_media(&self, file_path: &str, volume: f64, speed: f64, start_position_ms: Option<u64>, baked_rg: TrackReplayGain) -> Result<(), AppError> {
        (**self).play_media(file_path, volume, speed, start_position_ms, baked_rg)
    }
    fn resume(&self) { (**self).resume(); }
    fn pause(&self) { (**self).pause(); }
    fn stop(&self) { (**self).stop(); }
    fn seek(&self, position_ms: u64) { (**self).seek(position_ms); }
    fn set_volume(&self, volume: f64) { (**self).set_volume(volume); }
    fn set_speed(&self, speed: f64) { (**self).set_speed(speed); }
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) { (**self).preload_gapless(file_path, baked_rg); }
}

impl PlayerBackend for RodioPlayer {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError> {
        self.play_media(file_path, volume, speed, start_position_ms, baked_rg)
    }
    fn resume(&self) { self.resume(); }
    fn pause(&self) { self.pause(); }
    fn stop(&self) { self.stop(); }
    fn seek(&self, position_ms: u64) { self.seek(position_ms); }
    fn set_volume(&self, volume: f64) { self.set_volume(volume); }
    fn set_speed(&self, speed: f64) { self.set_speed(speed); }
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) { self.preload_gapless(file_path, baked_rg); }
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

/// Convert rodio's reported position into the media (source) position.
///
/// rodio inserts `track_position()` *after* `speed()` in its source chain, so
/// `Player::get_pos()` measures the speed-adjusted *output* timeline:
/// `output = media / speed`. We therefore multiply by `speed` to recover the
/// media position the UI displays. See `seek_to_media` for the inverse.
///
/// `speed` is in `0.25..=2.0`. Real-world track durations stay well below
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

/// Inverse of [`compute_position`]: convert a MEDIA position into the
/// output-timeline value `try_seek` expects (`media / speed`). Pure half of
/// [`RodioPlayer::seek_to_media`], split out for testability. A non-positive
/// `speed` (only possible from a corrupt value) passes through unchanged.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "media_ms / speed is non-negative, finite, and fits in u64 for any real audio duration"
)]
pub fn media_to_output_ms(media_ms: u64, speed: f64) -> u64 {
    if speed > 0.0 {
        (media_ms as f64 / speed) as u64
    } else {
        media_ms
    }
}

pub struct RodioPlayer {
    // Mutex groups multi-op sequences (clear → set_volume → append → play → seek)
    // atomically; rodio's Player is already Send+Sync on its own.
    player: std::sync::Mutex<Player>,
    gapless_pending: AtomicBool,
    // Lock-free graphic-EQ state. Shared by every `EqSource` we append, so a
    // live change applies to both the playing track and the gapless-preloaded
    // one. Mutated off the player lock; seeded at boot from persisted settings.
    eq: Arc<EqShared>,
    // Lock-free ReplayGain master state (enabled / mode / preamp / prevent-clip).
    // Shared by every `EqSource` like `eq`, so a live change applies to both the
    // playing and preloaded track; the *per-track* gain is baked per source, not
    // held here. Seeded at boot from persisted settings.
    rg: Arc<ReplayGainShared>,
}

impl RodioPlayer {
    pub fn new(mixer: &Mixer) -> Self {
        let player = Player::connect_new(mixer);
        player.pause();
        Self {
            player: std::sync::Mutex::new(player),
            gapless_pending: AtomicBool::new(false),
            eq: EqShared::new(false, &[0.0; equalizer::NUM_BANDS]),
            rg: ReplayGainShared::new(),
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
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError> {
        let player = self.lock_player();
        player.clear();
        player.set_volume(narrow_audio_param(volume));
        player.set_speed(narrow_audio_param(speed));
        let source = EqSource::new(decode_file(file_path)?, self.eq.clone(), self.rg.clone(), baked_rg);
        player.append(source);
        player.play();
        self.gapless_pending.store(false, Ordering::Release);
        if let Some(pos) = start_position_ms {
            log::debug!("Resuming playback at {pos}ms");
            Self::seek_to_media(&player, pos, speed);
        }
        Ok(())
    }

    /// Seek the (already-locked) player to a MEDIA-time position, accounting
    /// for the current speed.
    ///
    /// rodio tracks position *after* the speed stage, so `try_seek`'s argument
    /// is in the speed-adjusted *output* timeline (`Speed::try_seek` multiplies
    /// it back up by the factor to reach the decoder). Passing `media_ms`
    /// directly would seek the decoder to `media_ms × speed` and read back
    /// wrong via `get_pos() × speed`. We pass `media_ms / speed` so the decoder
    /// lands exactly on `media_ms` and the round-trip is consistent.
    fn seek_to_media(player: &Player, media_ms: u64, speed: f64) {
        let output_ms = media_to_output_ms(media_ms, speed);
        if let Err(e) = player.try_seek(Duration::from_millis(output_ms)) {
            log::warn!("Seek failed: {e}");
        }
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
        let speed = f64::from(player.speed());
        Self::seek_to_media(&player, position_ms, speed);
    }

    pub fn set_volume(&self, volume: f64) {
        let player = self.lock_player();
        player.set_volume(narrow_audio_param(volume));
    }

    pub fn set_speed(&self, speed: f64) {
        let player = self.lock_player();
        let old_speed = f64::from(player.speed());
        // Current media position under the OLD speed, captured before changing it.
        let media_ms = compute_position(player.get_pos(), old_speed);
        player.set_speed(narrow_audio_param(speed));
        // Re-anchor rodio's position tracker. Without this, `get_pos()` keeps
        // the output-time it accumulated at the old speed, and `query_position`'s
        // `get_pos() × new_speed` rescales that whole elapsed portion — so the
        // UI position would jump (e.g. 1×→2× doubles it, the bug this fixes).
        // Seeking resets the tracker's offset to the current media position in
        // the new speed's timeline so playback continues from where it is.
        // Skipped when nothing has played yet (boot / stopped at 0) to avoid a
        // spurious decoder seek.
        if media_ms > 0 {
            Self::seek_to_media(&player, media_ms, speed);
        }
    }

    /// Enable / disable the graphic equalizer. Lock-free — touches only the
    /// shared EQ state, not the player; the change is picked up by every
    /// `EqSource` (playing + preloaded) on its next sample.
    pub fn set_eq_enabled(&self, enabled: bool) {
        self.eq.set_enabled(enabled);
    }

    /// Set a single band's gain (dB). Out-of-range indices are ignored.
    pub fn set_eq_band(&self, index: usize, gain_db: f32) {
        self.eq.set_gain(index, gain_db);
    }

    /// Replace all band gains at once (preset / reset / boot hydration).
    pub fn set_eq_gains(&self, gains: &[f32]) {
        self.eq.set_all_gains(gains);
    }

    /// Set the EQ preamp / master gain (dB). Lock-free, like the other EQ setters.
    pub fn set_eq_preamp(&self, preamp_db: f32) {
        self.eq.set_preamp(preamp_db);
    }

    /// Enable / disable `ReplayGain`. Lock-free — touches only the shared RG state;
    /// every `EqSource` (playing + preloaded) picks it up on its next sample.
    pub fn set_replaygain_enabled(&self, enabled: bool) {
        self.rg.set_enabled(enabled);
    }

    /// Set the `ReplayGain` mode (Track / Album). Lock-free.
    pub fn set_replaygain_mode(&self, mode: RgMode) {
        self.rg.set_mode(mode);
    }

    /// Set the `ReplayGain` preamp (dB). Lock-free.
    pub fn set_replaygain_preamp(&self, preamp_db: f32) {
        self.rg.set_preamp(preamp_db);
    }

    /// Enable / disable the static peak-based clip guard. Lock-free.
    pub fn set_replaygain_prevent_clipping(&self, on: bool) {
        self.rg.set_prevent_clipping(on);
    }

    /// Whether a gapless source is currently staged behind the playing one.
    /// Used by the playback monitor to avoid re-issuing the late preload each tick.
    pub fn is_gapless_preloaded(&self) -> bool {
        self.gapless_pending.load(Ordering::Acquire)
    }

    pub fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) {
        match file_path {
            Some(path) => {
                // Decode (file open + Symphonia format probe — synchronous
                // I/O) *before* taking the Player lock: the playback
                // monitor's 500 ms position query shares this mutex, so a
                // probe under the lock stalls position publication right
                // when a preload fires near track end. Only the `append`
                // needs the lock; cross-action ordering against
                // `play_media` / `stop` is unchanged (the lock never
                // ordered whole actions, only made decode+append atomic).
                match decode_file(path) {
                    Ok(source) => {
                        // Wrap before locking — `EqSource::new` only reads the
                        // decoder's channel/rate, no I/O, so it stays out of the
                        // player-lock window the position monitor contends for.
                        let source = EqSource::new(source, self.eq.clone(), self.rg.clone(), baked_rg);
                        let player = self.lock_player();
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

    /// Query the current playback position in milliseconds (media timeline).
    /// `get_pos()` returns the speed-adjusted output time (`media / speed`);
    /// `compute_position` multiplies by speed to recover the media position.
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
/// 0.25..=2.0) to the `f32` Rodio's API expects.
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
