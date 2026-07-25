use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use rodio::mixer::Mixer;
use rodio::{Decoder, Player};

use crate::config::Paths;
use crate::error::AppError;

use super::crossfade::{self, CrossfadeShared};
use super::decks::{Deck, Decks, DeferredOp, lock_decks};
use super::equalizer::{self, EqShared, EqSource};
use super::replaygain::{ReplayGainShared, RgMode, TrackReplayGain};
use super::types::PersistableQueue;
use super::visualizer::{VisualizerShared, VisualizerTap};

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
    /// Start the next track on the idle deck and cross-fade the decks over
    /// `fade_ms` **media** milliseconds. The currently playing deck ends itself
    /// when its ramp lands.
    fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError>;
    fn resume(&self);
    /// Fade to silence over `fade_ms` and then pause. `0` is an immediate pause.
    /// There is no unconditional `pause()` here on purpose: `PlayerAction::Pause`
    /// always carries a length, so this is the only pause the action layer needs.
    fn pause_with_fade(&self, fade_ms: u64);
    fn stop(&self);
    /// Fade to silence over `fade_ms` and then stop. `0` is an immediate stop.
    fn stop_with_fade(&self, fade_ms: u64);
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
    fn begin_crossfade(&self, file_path: &str, baked_rg: TrackReplayGain, fade_ms: u64, volume: f64, speed: f64) -> Result<(), AppError> {
        (**self).begin_crossfade(file_path, baked_rg, fade_ms, volume, speed)
    }
    fn resume(&self) { (**self).resume(); }
    fn pause_with_fade(&self, fade_ms: u64) { (**self).pause_with_fade(fade_ms); }
    fn stop(&self) { (**self).stop(); }
    fn stop_with_fade(&self, fade_ms: u64) { (**self).stop_with_fade(fade_ms); }
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
    fn begin_crossfade(&self, file_path: &str, baked_rg: TrackReplayGain, fade_ms: u64, volume: f64, speed: f64) -> Result<(), AppError> {
        self.begin_crossfade(file_path, baked_rg, fade_ms, volume, speed)
    }
    fn resume(&self) { self.resume(); }
    fn pause_with_fade(&self, fade_ms: u64) { self.pause_with_fade(fade_ms); }
    fn stop(&self) { self.stop(); }
    fn stop_with_fade(&self, fade_ms: u64) { self.stop_with_fade(fade_ms); }
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
    // atomically; rodio's Player is already Send+Sync on its own. `Arc` so a
    // deferred pause/stop task can hold the decks after its sleep.
    decks: Arc<std::sync::Mutex<Decks>>,
    // `Arc` for the same reason as `deck_epoch`: the deferred clear of a faded
    // stop is the deferred half of `stop()`, so it must be able to drop the flag
    // alongside the deck contents it removes.
    gapless_pending: Arc<AtomicBool>,
    // Set while a crossfade's outgoing deck is still draining. Paired with the
    // idle deck's `empty()` in `is_crossfading`, which self-heals the flag.
    crossfade_armed: AtomicBool,
    // Bumped by every control op that replaces deck contents. Two readers, both
    // of which re-check it *under the decks lock* and bail if it moved, so a
    // newer op always wins: a deferred pause/stop after its sleep, and a
    // `preload_gapless` after its (unlocked) decode.
    deck_epoch: Arc<AtomicU64>,
    // Lock-free graphic-EQ state. Shared by every `EqSource` we append, so a
    // live change applies to both the playing track and the gapless-preloaded
    // one. Mutated off the player lock; seeded at boot from persisted settings.
    eq: Arc<EqShared>,
    // Lock-free ReplayGain master state (enabled / mode / preamp / prevent-clip).
    // Shared by every `EqSource` like `eq`, so a live change applies to both the
    // playing and preloaded track; the *per-track* gain is baked per source, not
    // held here. Seeded at boot from persisted settings.
    rg: Arc<ReplayGainShared>,
    // Lock-free crossfade settings, read by the control layer (this backend and
    // the playback monitor) — never by the audio thread, so no generation counter.
    xf: Arc<CrossfadeShared>,
    // Lock-free sample ring for the audio visualizer. Written by every source we
    // append (see `build_source`) and read by the UI. Constructed disarmed and
    // armed from `settings.json` by `hydrate_audio_dsp` — which, alone among the
    // audio features, ships it *on* (see `VisualizerFlags`). Disarmed it is a
    // no-op on the audio thread.
    viz: Arc<VisualizerShared>,
    // Used only to schedule the deferred half of a faded pause / stop.
    runtime: tokio::runtime::Handle,
}

impl RodioPlayer {
    pub fn new(mixer: &Mixer, runtime: tokio::runtime::Handle) -> Self {
        Self {
            decks: Arc::new(std::sync::Mutex::new(Decks::connect(mixer))),
            gapless_pending: Arc::new(AtomicBool::new(false)),
            crossfade_armed: AtomicBool::new(false),
            deck_epoch: Arc::new(AtomicU64::new(0)),
            eq: EqShared::new(false, &[0.0; equalizer::NUM_BANDS]),
            rg: ReplayGainShared::new(),
            xf: CrossfadeShared::new(),
            viz: VisualizerShared::new(false),
            runtime,
        }
    }

    /// Invalidate any deferred pause/stop *and* any in-flight gapless preload,
    /// then return the new epoch. Every control op that replaces deck contents
    /// calls this.
    fn bump_epoch(&self) -> u64 {
        self.deck_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Run `op` once `delay_ms` has elapsed, unless a newer control op bumped
    /// the epoch in the meantime. The epoch is re-checked *while holding the
    /// decks lock* so a concurrent `play_media` can't be clobbered by a
    /// deferred clear that had already passed a lock-free check.
    fn schedule_after(&self, epoch: u64, delay_ms: u64, op: DeferredOp) {
        let decks = self.decks.clone();
        let deck_epoch = self.deck_epoch.clone();
        let gapless_pending = self.gapless_pending.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            let guard = lock_decks(&decks);
            if deck_epoch.load(Ordering::Acquire) != epoch {
                return;
            }
            match op {
                // A pause keeps the deck contents — a staged gapless source
                // included — so it must leave `gapless_pending` alone.
                DeferredOp::PauseAll => guard.pause_all(),
                // The deferred half of `stop()`, so it drops the flag exactly as
                // `stop()` does: *after* the decks, never before. A preload that
                // enters after the epoch bump snapshots the *new* epoch, passes
                // its own re-check and can still stage a source behind the deck
                // this is about to throw away — clearing the flag here is what
                // stops it outliving the source it describes. A preload that
                // instead lands *after* this clear is refused by
                // `preload_gapless`'s empty-deck gate, which is what keeps the
                // flag from being resurrected over decks we just emptied.
                DeferredOp::ClearAll => {
                    guard.clear_all();
                    gapless_pending.store(false, Ordering::Release);
                }
            }
        });
    }

    /// Is the active deck actually playing something we could fade out of? See
    /// [`Deck::busy`] for why a paused or empty deck can't carry a ramp.
    fn active_deck_busy(&self) -> bool {
        self.lock_decks().active().busy()
    }

    /// Can a fade-out ramp actually run on the active deck right now? The shared
    /// gate of [`Self::pause_with_fade`] and [`Self::stop_with_fade`] — four
    /// terms, every one of which falls through to the immediate op:
    ///
    /// - **`fade_ms == 0`** — the caller asked for an immediate op. Checked first,
    ///   so `stop_end_of_queue` never even takes the deck lock.
    /// - **an idle deck** — see [`Self::active_deck_busy`]. `player_stop` passes the
    ///   pause-fade length whatever the player is doing (`build_stop_actions`
    ///   doesn't look at the status), so a stop routinely lands on a paused deck.
    /// - **a staged gapless source** — it shares this deck's fade cell, so the
    ///   moment the outgoing source drains it would inherit the ramp and burn its
    ///   own first `fade_ms` fading to silence. Same gate, and the same reason, as
    ///   [`crossfade::manual_fade_ms`].
    /// - **a crossfade in flight** — the outgoing deck's ramp has no start gain we
    ///   could restore it to on resume. `Pausable` stops pulling the source
    ///   entirely, so both ramps simply freeze.
    fn can_fade_out(&self, fade_ms: u64) -> bool {
        fade_ms > 0
            && self.active_deck_busy()
            && !self.is_gapless_preloaded()
            && !self.is_crossfading()
    }

    /// Fade length for a *manual* track change, or `0` for a hard cut.
    /// Gathers the live inputs; the decision itself is
    /// [`crossfade::manual_fade_ms`], which documents each gate.
    ///
    /// The `gapless_pending` read here is advisory: the monitor's preload is
    /// *not* taken under the `exec_lock` that serializes player actions, so it
    /// can land (or be mid-decode) while we decide. `play_media` re-checks the
    /// flag under the deck lock before committing to the fade, and the preload
    /// itself re-checks the deck epoch before staging — so whichever of the two
    /// commits second yields.
    fn manual_fade_ms(&self, start_position_ms: Option<u64>) -> u64 {
        crossfade::manual_fade_ms(
            self.xf.snapshot(),
            start_position_ms.is_some(),
            self.active_deck_busy(),
            self.is_gapless_preloaded(),
        )
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
        let fade_ms = self.manual_fade_ms(start_position_ms);

        // Decode (file open + Symphonia probe — synchronous I/O) *before* taking
        // the deck lock, mirroring `preload_gapless`. The position monitor's
        // `query_position` shares this mutex, so a probe under the lock stalls
        // position publication. Everything that depends on *which* deck we land
        // on is decided below, under the one lock we then act through — the
        // decode is the only step that can be hoisted out of it.
        let decoded = decode_file(file_path)?;

        self.bump_epoch();
        let mut decks = self.lock_decks();
        // Backstop for the gapless race: the monitor's `preload_gapless` sets
        // `gapless_pending` while holding this same lock, so a preload that
        // landed during the decode above is visible here. Downgrading to a hard
        // cut is always safe — it clears both decks, staged source included.
        let fade_ms = if self.gapless_pending.load(Ordering::Acquire) { 0 } else { fade_ms };
        // The source is built by a closure the deck primitives hand the *target*
        // deck, under the one lock they then append through — so the ramp cell it
        // carries always belongs to the deck it lands on. `EqSource::new` does no
        // I/O (it only reads the decoder's channel count and sample rate), so
        // building it there costs the lock nothing.
        let build = |deck: &Deck| self.build_source(decoded, baked_rg, deck);
        if fade_ms > 0 {
            decks.crossfade_to(fade_ms, volume, speed, build);
            self.crossfade_armed.store(true, Ordering::Release);
        } else {
            decks.cut_to(volume, speed, build);
            self.crossfade_armed.store(false, Ordering::Release);
        }
        self.gapless_pending.store(false, Ordering::Release);
        if let Some(pos) = start_position_ms {
            log::debug!("Resuming playback at {pos}ms");
            Self::seek_to_media(&decks.active().player, pos, speed);
        }
        Ok(())
    }

    /// Start `file_path` on the idle deck and cross-fade the two decks over
    /// `fade_ms` media milliseconds. The outgoing deck's source ends itself
    /// when its ramp lands (`end_on_complete`), draining that deck — which is
    /// how [`Self::is_crossfading`] knows the overlap is over.
    ///
    /// A decode failure returns before any deck is touched, so the outgoing
    /// track keeps playing and the caller can fall back to a plain skip.
    pub fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError> {
        // Decode outside the lock; pick the deck and build the source inside it,
        // so the ramp cell the source carries is guaranteed to belong to the deck
        // it is appended to. See the same note in `play_media`.
        let decoded = decode_file(file_path)?;

        self.bump_epoch();
        let mut decks = self.lock_decks();
        // A staged gapless source would sit on the *active* deck and inherit its
        // fade cell, so it would fade out alongside the track it was meant to
        // follow. `crossfade_eligible` gates the preload off for exactly this
        // reason; assert the invariant rather than silently mis-fading.
        debug_assert!(
            !self.gapless_pending.load(Ordering::Acquire),
            "crossfade must never race a staged gapless preload"
        );
        decks.crossfade_to(fade_ms, volume, speed, |deck| {
            self.build_source(decoded, baked_rg, deck)
        });
        self.crossfade_armed.store(true, Ordering::Release);
        self.gapless_pending.store(false, Ordering::Release);
        Ok(())
    }

    /// Wrap a decoded track in the audio source the decks play: the graphic EQ,
    /// this track's baked `ReplayGain`, and `deck`'s crossfade ramp cell — then
    /// the visualizer tap, which reads the finished signal without altering it.
    ///
    /// Always called with the deck the source is about to be appended to — see
    /// the module doc of [`super::decks`] for why the two can't be split.
    fn build_source(
        &self,
        decoded: Decoder<BufReader<File>>,
        baked_rg: TrackReplayGain,
        deck: &Deck,
    ) -> VisualizerTap<EqSource<Decoder<BufReader<File>>>> {
        VisualizerTap::new(
            EqSource::new(
                decoded,
                self.eq.clone(),
                self.rg.clone(),
                baked_rg,
                deck.fade.clone(),
            ),
            self.viz.clone(),
        )
    }

    /// Whether a crossfade's outgoing deck is still audible.
    ///
    /// `sound_count` is incremented synchronously inside `Player::append`, so a
    /// deck that was just handed a source never reports empty. The flag is
    /// cleared here once the outgoing deck drains, so this doubles as the
    /// crossfade's completion hook — no `Drop` impl or extra atomic needed.
    pub fn is_crossfading(&self) -> bool {
        if !self.crossfade_armed.load(Ordering::Acquire) {
            return false;
        }
        let decks = self.lock_decks();
        if decks.idle().player.empty() {
            drop(decks);
            self.crossfade_armed.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// Cancel an in-flight crossfade: drop the outgoing deck and ramp the
    /// survivor back to unity from wherever its fade-in reached. Called before
    /// a seek, which would otherwise leave the new track stuck at partial gain.
    fn abort_crossfade(&self) {
        if !self.crossfade_armed.swap(false, Ordering::AcqRel) {
            return;
        }
        let decks = self.lock_decks();
        decks.idle().reset();
        decks.active().fade.arm(None, 1.0, crossfade::ABORT_RAMP_MS, false);
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
        self.bump_epoch();
        // A crossfade in flight owns both decks' ramps; re-arming the active
        // one here would restart its fade-in from silence.
        let crossfading = self.is_crossfading();
        let decks = self.lock_decks();
        decks.play_all();
        if !crossfading {
            // Unconditional, not gated on the setting: a faded pause leaves the
            // deck holding silence, and the user may have turned the setting off
            // in between. A zero-length ramp just snaps back to unity.
            let ramp = if self.xf.fade_on_pause() { crossfade::PAUSE_FADE_MS } else { 0 };
            decks.active().fade.arm(None, 1.0, ramp, false);
        }
    }

    /// Pause both decks now. The caller decides whether a fade is wanted; see
    /// [`Self::pause_with_fade`].
    pub fn pause(&self) {
        self.bump_epoch();
        self.lock_decks().pause_all();
    }

    /// Arm a fade to silence on the active deck and schedule `op` to land once
    /// the ramp has. Returns `false` when the fade was refused — the caller then
    /// runs the immediate op instead.
    ///
    /// The shared prologue of [`Self::pause_with_fade`] and [`Self::stop_with_fade`];
    /// the two differ only in the deferred half. The ramp holds at zero rather
    /// than ending its source: a self-ending fade would drain the active deck and
    /// read as `EndOfStream`.
    fn arm_fade_out(&self, fade_ms: u64, op: DeferredOp) -> bool {
        if !self.can_fade_out(fade_ms) {
            return false;
        }
        let epoch = self.bump_epoch();
        {
            let decks = self.lock_decks();
            // Backstop for the gapless race, mirroring `play_media`. The read in
            // `can_fade_out` is lock-free and the bump above is a separate step,
            // so a preload can *complete* — append its source and set the flag,
            // both under this same lock — in between the two. This re-read is what
            // sees it. A preload arriving *after* the bump is the benign case: on
            // the pause path it stages behind a deck this fade only holds at
            // silence and then pauses, and the resume ramps that deck back to
            // unity; on the stop path the deferred `ClearAll` takes the staged
            // source and the flag together.
            if self.gapless_pending.load(Ordering::Acquire) {
                return false;
            }
            decks.active().fade.arm(None, 0.0, fade_ms, false);
        }
        self.schedule_after(epoch, fade_ms, op);
        true
    }

    /// Fade to silence over `fade_ms`, then pause both decks. Falls through to an
    /// immediate pause whenever [`Self::arm_fade_out`] refuses.
    pub fn pause_with_fade(&self, fade_ms: u64) {
        if !self.arm_fade_out(fade_ms, DeferredOp::PauseAll) {
            self.pause();
        }
    }

    /// Stop playback. `Player::clear()` removes all sources and pauses automatically.
    pub fn stop(&self) {
        self.bump_epoch();
        self.crossfade_armed.store(false, Ordering::Release);
        let decks = self.lock_decks();
        decks.clear_all();
        self.gapless_pending.store(false, Ordering::Release);
    }

    /// Fade to silence over `fade_ms`, then clear both decks. Falls through to an
    /// immediate stop whenever [`Self::arm_fade_out`] refuses. The deferred
    /// [`DeferredOp::ClearAll`] is what actually empties the decks.
    ///
    /// `gapless_pending` is deliberately *not* cleared here. The gate has already
    /// refused the fade if a source was staged, so on this path the flag is false
    /// — and clearing it eagerly would only be a way to start lying about a source
    /// the deferred clear has not removed yet. A preload that still slips in
    /// behind us stages a real source, and the deferred `ClearAll` drops it and
    /// the flag together; one that slips in behind *the clear* finds an empty deck
    /// and is refused outright.
    pub fn stop_with_fade(&self, fade_ms: u64) {
        if !self.arm_fade_out(fade_ms, DeferredOp::ClearAll) {
            self.stop();
        }
    }

    pub fn seek(&self, position_ms: u64) {
        self.abort_crossfade();
        // Deliberately does NOT bump the fade epoch. A seek doesn't replace deck
        // contents, and cancelling a pending deferred pause here would leave the
        // decks running (silently, at the pause ramp's zero gain) while the UI
        // reads Paused — seeking a paused track is an ordinary thing to do.
        let decks = self.lock_decks();
        let player = &decks.active().player;
        let speed = f64::from(player.speed());
        Self::seek_to_media(player, position_ms, speed);
    }

    pub fn set_volume(&self, volume: f64) {
        self.lock_decks().set_volume_all(volume);
    }

    pub fn set_speed(&self, speed: f64) {
        let decks = self.lock_decks();
        let player = &decks.active().player;
        let old_speed = f64::from(player.speed());
        // Current media position under the OLD speed, captured before changing it.
        let media_ms = compute_position(player.get_pos(), old_speed);
        // Both decks must run at the same speed or a crossfade would drift, but
        // only the active one carries a position worth re-anchoring.
        decks.set_speed_all(speed);
        // The visualizer taps the sources *under* rodio's speed stage, so its
        // analyzer needs the factor to place band edges on the pitch you hear.
        // This is that cell's only writer — see `VisualizerShared::set_speed`.
        self.viz.set_speed(speed);
        // Re-anchor rodio's position tracker. Without this, `get_pos()` keeps
        // the output-time it accumulated at the old speed, and `query_position`'s
        // `get_pos() × new_speed` rescales that whole elapsed portion — so the
        // UI position would jump (e.g. 1×→2× doubles it, the bug this fixes).
        // Seeking resets the tracker's offset to the current media position in
        // the new speed's timeline so playback continues from where it is.
        // Skipped when nothing has played yet (boot / stopped at 0) to avoid a
        // spurious decoder seek.
        if media_ms > 0 {
            Self::seek_to_media(player, media_ms, speed);
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

    /// Enable / disable the visualizer's sample tap. Lock-free, like the EQ and
    /// `ReplayGain` setters — while it is off the tap never touches the ring.
    pub fn set_visualizer_enabled(&self, on: bool) {
        self.viz.set_enabled(on);
    }

    /// The visualizer's sample ring, for the UI-side analyzer to snapshot.
    pub fn visualizer(&self) -> Arc<VisualizerShared> {
        self.viz.clone()
    }

    /// Whether a gapless source is currently staged behind the playing one.
    /// Used by the playback monitor to avoid re-issuing the late preload each tick.
    pub fn is_gapless_preloaded(&self) -> bool {
        self.gapless_pending.load(Ordering::Acquire)
    }

    /// Stage the next track *behind* the current one on the **active** deck, so
    /// rodio's queue plays them back-to-back with no gap. This is the gapless
    /// path, and it is mutually exclusive with a crossfade for a given
    /// transition — see `crossfade::crossfade_eligible`.
    pub fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) {
        let Some(path) = file_path else {
            // The action layer only ever *clears* a stale preload through here.
            self.gapless_pending.store(false, Ordering::Release);
            return;
        };

        // Snapshot the deck epoch before the (unlocked) decode. Every op that
        // replaces deck contents — `play_media`, `begin_crossfade`, `stop`,
        // `pause`, … — bumps it, so a mismatch below means the decks moved out
        // from under this preload and the track we staged for is no longer the one
        // playing. Staging it anyway would queue the wrong track *and*, if the
        // active deck flipped, hand it the other deck's ramp cell (which may be
        // armed to fade out and end).
        let epoch = self.deck_epoch.load(Ordering::Acquire);

        // Decode (file open + Symphonia format probe — synchronous I/O) *before*
        // taking the deck lock: the playback monitor's position query shares this
        // mutex, so a probe under the lock stalls position publication right when
        // a preload fires near track end.
        let decoded = match decode_file(path) {
            Ok(decoded) => decoded,
            Err(e) => {
                log::warn!("Failed to preload gapless track {path}: {e}");
                self.gapless_pending.store(false, Ordering::Release);
                return;
            }
        };

        // Re-check the epoch, then stage — all under one lock. `Deck::stage` takes
        // the builder rather than a source, so the ramp cell the source carries is
        // the one belonging to the deck it lands on, as with the other two appends.
        let decks = self.lock_decks();
        if self.deck_epoch.load(Ordering::Acquire) != epoch {
            log::debug!("Dropping stale gapless preload of {path}");
            return;
        }
        let deck = decks.active();
        // A *gapless* preload only means anything staged behind a source that is
        // still there to be followed. The epoch can't say that on its own:
        // `stop()` and the deferred [`DeferredOp::ClearAll`] empty the decks
        // *without* bumping it, so a preload whose decode outran either one would
        // pass the re-check above, append behind nothing, and set
        // `gapless_pending` over a deck the stop already cleared — the exact state
        // `stop()` clears the flag to forbid. `check_playback_state` would then
        // read that as a `GaplessTransition` off an empty deck.
        //
        // Never rejects a legitimate stage: the only caller that passes a path is
        // the monitor's late preload, which runs solely in its `Playing` branch,
        // and `play_media` appends under this same lock, so the deck is never
        // transiently empty behind it.
        if deck.player.empty() {
            log::debug!("Dropping gapless preload of {path}: nothing left to follow");
            return;
        }
        deck.stage(|d| self.build_source(decoded, baked_rg, d));
        self.gapless_pending.store(true, Ordering::Release);
    }

    /// Query the current playback position in milliseconds (media timeline).
    /// `get_pos()` returns the speed-adjusted output time (`media / speed`);
    /// `compute_position` multiplies by speed to recover the media position.
    pub fn query_position(&self) -> u64 {
        let decks = self.lock_decks();
        let player = &decks.active().player;
        let wall_time = player.get_pos();
        let speed = f64::from(player.speed());
        compute_position(wall_time, speed)
    }

    /// Check playback state in a single lock acquisition to avoid TOCTOU races
    /// between gapless transition detection and end-of-stream detection.
    ///
    /// Reads the **active** deck only. During a crossfade that deck holds
    /// exactly the incoming track, so this reports `Playing` — the outgoing
    /// deck draining is watched by [`Self::is_crossfading`] instead.
    pub fn check_playback_state(&self) -> PlaybackCheck {
        let was_gapless = self.gapless_pending.load(Ordering::Acquire);
        let decks = self.lock_decks();
        let player = &decks.active().player;
        let queue_len = player.len();
        let is_empty = player.empty();
        drop(decks);

        let result = evaluate_playback_check(was_gapless, queue_len, is_empty);
        if result == PlaybackCheck::GaplessTransition {
            self.gapless_pending.store(false, Ordering::Release);
        }
        result
    }

    /// Snapshot the live crossfade settings for the playback monitor's decision.
    pub fn crossfade_settings(&self) -> crossfade::CrossfadeSettings {
        self.xf.snapshot()
    }

    /// Enable / disable crossfade. Lock-free, like the EQ and `ReplayGain` setters.
    pub fn set_crossfade_enabled(&self, enabled: bool) {
        self.xf.set_enabled(enabled);
    }

    /// Set the crossfade length (ms), clamped to the supported range.
    pub fn set_crossfade_duration_ms(&self, ms: u32) {
        self.xf.set_duration_ms(ms);
    }

    /// Also crossfade when the user changes track manually.
    pub fn set_crossfade_manual(&self, on: bool) {
        self.xf.set_manual(on);
    }

    /// Leave same-album transitions gapless.
    pub fn set_crossfade_skip_same_album(&self, on: bool) {
        self.xf.set_skip_same_album(on);
    }

    /// Fade out on pause / user stop, fade back in on resume.
    pub fn set_crossfade_fade_on_pause(&self, on: bool) {
        self.xf.set_fade_on_pause(on);
    }

    /// Lock the decks mutex, recovering from poison rather than panicking.
    fn lock_decks(&self) -> std::sync::MutexGuard<'_, Decks> {
        lock_decks(&self.decks)
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

#[cfg(test)]
#[path = "tests/rodio_backend_tests.rs"]
mod tests;
