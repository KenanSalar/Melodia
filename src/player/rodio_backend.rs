use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use rodio::mixer::Mixer;
use rodio::{Decoder, Player, Source};

use crate::error::AppError;

use super::crossfade::{self, CrossfadeShared};
use super::decks::{Deck, Decks, DeferredOp, lock_decks};
use super::equalizer::{self, EqShared, EqSource};
use super::prebuffer::StreamShared;
use super::replaygain::{ReplayGainShared, RgMode, TrackReplayGain};
use super::stream_source::PreparedStream;
use super::visualizer::{VisualizerShared, VisualizerTap};

/// Audio playback operations, so tests can stand a mock in for `RodioPlayer`.
pub trait PlayerBackend: Send + Sync {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError>;
    /// Start the next track on the idle deck and cross-fade over `fade_ms`
    /// **media** milliseconds; the outgoing deck ends itself when its ramp lands.
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
    /// `PlayerAction::Pause` always carries a length, so the action layer needs
    /// no unconditional `pause()` beside this.
    fn pause_with_fade(&self, fade_ms: u64);
    fn stop(&self);
    /// Fade to silence over `fade_ms` and then stop. `0` is an immediate stop.
    fn stop_with_fade(&self, fade_ms: u64);
    fn seek(&self, position_ms: u64);
    fn set_volume(&self, volume: f64);
    fn set_speed(&self, speed: f64);
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain);
    /// Start the live stream staged under `generation`, hard-cutting whatever was playing.
    ///
    /// Takes no path and no `ReplayGain`: the stream was opened asynchronously long before this
    /// runs (a socket has no business on the action executor's thread), and a live source carries
    /// no per-track tags to bake. Fails when nothing is staged under that generation, which is how
    /// a station superseded mid-connect is refused rather than played late.
    fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError>;
}

/// Blanket impl so an `Arc<RodioPlayer>` is itself a backend.
impl<T: std::ops::Deref + Send + Sync> PlayerBackend for T
where
    T::Target: PlayerBackend,
{
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError> {
        (**self).play_media(file_path, volume, speed, start_position_ms, baked_rg)
    }
    fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError> {
        (**self).begin_crossfade(file_path, baked_rg, fade_ms, volume, speed)
    }
    fn resume(&self) {
        (**self).resume();
    }
    fn pause_with_fade(&self, fade_ms: u64) {
        (**self).pause_with_fade(fade_ms);
    }
    fn stop(&self) {
        (**self).stop();
    }
    fn stop_with_fade(&self, fade_ms: u64) {
        (**self).stop_with_fade(fade_ms);
    }
    fn seek(&self, position_ms: u64) {
        (**self).seek(position_ms);
    }
    fn set_volume(&self, volume: f64) {
        (**self).set_volume(volume);
    }
    fn set_speed(&self, speed: f64) {
        (**self).set_speed(speed);
    }
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) {
        (**self).preload_gapless(file_path, baked_rg);
    }
    fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError> {
        (**self).play_stream(generation, volume)
    }
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
    fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError> {
        self.begin_crossfade(file_path, baked_rg, fade_ms, volume, speed)
    }
    fn resume(&self) {
        self.resume();
    }
    fn pause_with_fade(&self, fade_ms: u64) {
        self.pause_with_fade(fade_ms);
    }
    fn stop(&self) {
        self.stop();
    }
    fn stop_with_fade(&self, fade_ms: u64) {
        self.stop_with_fade(fade_ms);
    }
    fn seek(&self, position_ms: u64) {
        self.seek(position_ms);
    }
    fn set_volume(&self, volume: f64) {
        self.set_volume(volume);
    }
    fn set_speed(&self, speed: f64) {
        self.set_speed(speed);
    }
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) {
        self.preload_gapless(file_path, baked_rg);
    }
    fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError> {
        self.play_stream(generation, volume)
    }
}

/// Result of checking the Rodio player queue in a single lock acquisition.
#[derive(Debug, PartialEq)]
pub enum PlaybackCheck {
    /// Queue depth dropped from 2 to 1 — the staged source took over.
    GaplessTransition,
    EndOfStream,
    Playing,
}

/// Pure half of [`RodioPlayer::check_playback_state`], split out for testability.
pub fn evaluate_playback_check(
    was_gapless: bool,
    queue_len: usize,
    is_empty: bool,
) -> PlaybackCheck {
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
/// rodio inserts `track_position()` *after* `speed()`, so `Player::get_pos()`
/// measures the *output* timeline (`media / speed`) and recovering the media
/// position means multiplying back up. [`media_to_output_ms`] is the inverse.
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

/// Inverse of [`compute_position`]: a MEDIA position as the output-timeline
/// value `try_seek` expects. A non-positive `speed` passes through unchanged.
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
    // The mutex is what makes a multi-op sequence atomic (rodio's `Player` is
    // already Send+Sync); `Arc` so a deferred pause/stop can hold the decks
    // after its sleep.
    decks: Arc<std::sync::Mutex<Decks>>,
    // `Arc` because the deferred clear of a faded stop must drop this flag
    // alongside the deck contents it removes.
    gapless_pending: Arc<AtomicBool>,
    // Set while a crossfade's outgoing deck is still draining. Paired with the
    // idle deck's `empty()` in `is_crossfading`, which self-heals the flag.
    crossfade_armed: AtomicBool,
    // Bumped by every control op that replaces deck contents, so a newer op
    // always wins. Its two readers — a deferred pause/stop after its sleep, and
    // a `preload_gapless` after its unlocked decode — re-check it *under the
    // decks lock* and bail if it moved.
    deck_epoch: Arc<AtomicU64>,
    // Lock-free EQ / ReplayGain master state, shared by every `EqSource` we
    // append so a live change reaches the playing *and* the preloaded track.
    // The per-track RG gain is baked per source, not held here. Both are seeded
    // at boot from persisted settings.
    eq: Arc<EqShared>,
    rg: Arc<ReplayGainShared>,
    // Read by the control layer only — never by the audio thread, hence no
    // generation counter.
    xf: Arc<CrossfadeShared>,
    // One sample ring per deck, written by every source we append and read by
    // the UI as their sum — which is what makes a crossfade read as the mix
    // rather than as two interleaved tracks. Unlike the cells above it is *not*
    // seeded at boot: it stays disarmed (a no-op) until the Now-Playing view is
    // on screen, so the audio thread never fills a ring nobody reads.
    viz: Arc<VisualizerShared>,
    // A live stream opened asynchronously and waiting for its `PlayStream` action, tagged with the
    // station generation it was opened for. Staged rather than carried on the action because
    // `PlayerAction` is plain `Clone + PartialEq` data and a decoder is neither — the same reason
    // `preload_gapless` takes its decode through a side channel. Dropping a superseded stage
    // closes its connection, so a station started while another was still connecting cancels the
    // loser for free.
    staged_stream: Mutex<Option<(u64, PreparedStream)>>,
    // The cell the playing stream publishes buffering and live titles through, or `None` when the
    // source is a local file. Doubles as the monitor's "is this a station?" test, which is why it
    // lives here rather than being re-read off `PlayerState`. `Arc` for the reason
    // `gapless_pending` is one: the deferred clear of a faded stop has to drop this alongside the
    // deck contents it removes.
    live_stream: Arc<Mutex<Option<Arc<StreamShared>>>>,
    // Only ever schedules the deferred half of a faded pause / stop.
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
            staged_stream: Mutex::new(None),
            live_stream: Arc::new(Mutex::new(None)),
            runtime,
        }
    }

    /// Invalidate any deferred pause/stop *and* any in-flight gapless preload,
    /// returning the new epoch. Every op that replaces deck contents calls this.
    fn bump_epoch(&self) -> u64 {
        self.deck_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Run `op` after `delay_ms`, unless a newer control op bumped the epoch in
    /// the meantime. Re-checked *under the decks lock* so a concurrent
    /// `play_media` can't be clobbered by a clear that passed a lock-free check.
    fn schedule_after(&self, epoch: u64, delay_ms: u64, op: DeferredOp) {
        let decks = self.decks.clone();
        let deck_epoch = self.deck_epoch.clone();
        let gapless_pending = self.gapless_pending.clone();
        let live_stream = self.live_stream.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            let guard = lock_decks(&decks);
            if deck_epoch.load(Ordering::Acquire) != epoch {
                return;
            }
            match op {
                // A pause keeps the deck contents — staged gapless source
                // included — so it must leave `gapless_pending` alone.
                DeferredOp::PauseAll => guard.pause_all(),
                // The deferred half of `stop()`, so it drops the flag the same
                // way: *after* the decks, never before. A preload that entered
                // behind the epoch bump can still have staged a source onto the
                // deck this throws away, and clearing here is what stops the
                // flag outliving it; one arriving after is refused by
                // `preload_gapless`'s empty-deck gate.
                DeferredOp::ClearAll => {
                    guard.clear_all();
                    gapless_pending.store(false, Ordering::Release);
                    // The faded half of a stop, so it drops the live-stream cell alongside the
                    // source it removes — the same pairing the flag above gets.
                    *live_stream.lock() = None;
                }
            }
        });
    }

    /// Is the active deck actually playing something we could fade out of? See
    /// [`Deck::busy`] for why a paused or empty deck can't carry a ramp.
    fn active_deck_busy(&self) -> bool {
        self.lock_decks().active().busy()
    }

    /// The shared gate of [`Self::pause_with_fade`] and [`Self::stop_with_fade`];
    /// each term falls through to the immediate op:
    ///
    /// - **`fade_ms == 0`** — checked first, so `stop_end_of_queue` never takes
    ///   the deck lock at all.
    /// - **an idle deck** — `player_stop` passes the pause-fade length whatever
    ///   the status, so a stop routinely lands on a paused deck.
    /// - **a staged gapless source** — it shares this deck's fade cell and would
    ///   inherit the ramp the moment the outgoing source drained. Same gate, and
    ///   reason, as [`crossfade::manual_fade_ms`].
    /// - **a crossfade in flight** — the outgoing ramp has no start gain to
    ///   restore on resume; `Pausable` just freezes both.
    fn can_fade_out(&self, fade_ms: u64) -> bool {
        fade_ms > 0
            && self.active_deck_busy()
            && !self.is_gapless_preloaded()
            && !self.is_crossfading()
    }

    /// Fade length for a *manual* track change, or `0` for a hard cut. Gathers
    /// the live inputs; [`crossfade::manual_fade_ms`] documents each gate.
    ///
    /// The `gapless_pending` read is advisory — the monitor's preload isn't
    /// under the `exec_lock`, so it can land mid-decision. `play_media`
    /// re-checks under the deck lock and the preload re-checks the epoch before
    /// staging, so whichever commits second yields.
    fn manual_fade_ms(&self, start_position_ms: Option<u64>) -> u64 {
        crossfade::manual_fade_ms(
            self.xf.snapshot(),
            start_position_ms.is_some(),
            self.active_deck_busy(),
            self.is_gapless_preloaded(),
        )
    }

    /// Start playback of a file, optionally seeking — all under one lock, so the
    /// playback monitor can't observe position ~0 between the play and the seek.
    pub fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError> {
        let fade_ms = self.manual_fade_ms(start_position_ms);

        // Decode outside the deck lock — the position monitor's `query_position`
        // shares this mutex, so a synchronous Symphonia probe under it stalls
        // position publication. It is the only step that can be hoisted out;
        // everything depending on *which* deck we land on happens below.
        let decoded = decode_file(file_path)?;

        self.bump_epoch();
        self.clear_live_stream();
        let mut decks = self.lock_decks();
        // Backstop for the gapless race: `preload_gapless` sets the flag under
        // this same lock, so a preload that landed during the decode is visible
        // here. Downgrading to a hard cut is always safe — it clears both decks,
        // staged source included.
        let fade_ms = if self.gapless_pending.load(Ordering::Acquire) {
            0
        } else {
            fade_ms
        };
        // The deck primitives hand this closure the *target* deck under the one
        // lock they append through, so the ramp cell the source carries always
        // belongs to the deck it lands on. `EqSource::new` does no I/O, so
        // building there costs the lock nothing.
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

    /// Playback speed while a station plays.
    ///
    /// rodio implements speed by reporting a multiplied sample rate upward, which against a source
    /// arriving at a fixed real-time rate drifts the ring until it starves. Pinned rather than
    /// merely ignored, so what the deck runs at and what the transport claims stay the same
    /// number — [`super::state::PlayerState::build_station_connecting_actions`] resets the state
    /// side to match.
    const STREAM_SPEED: f64 = 1.0;

    /// Park an opened live stream for the `PlayStream` action that follows it.
    ///
    /// Replacing an unclaimed stage drops it, and dropping it closes its connection — which is how
    /// a station started while another was still connecting cancels the loser.
    pub fn stage_stream(&self, generation: u64, prepared: PreparedStream) {
        *self.staged_stream.lock() = Some((generation, prepared));
    }

    /// Drop the stage if it is still the one opened for `generation`.
    ///
    /// The transport can end a station's session between the open being started and finishing, and
    /// the state machine then refuses the `PlayStream` that would have claimed it. Without this the
    /// connection would stay open until some later station happened to stage over it.
    pub fn discard_staged_stream(&self, generation: u64) {
        let mut staged = self.staged_stream.lock();
        if staged.as_ref().is_some_and(|(staged_generation, _)| *staged_generation == generation) {
            *staged = None;
        }
    }

    /// The cell a playing station publishes buffering and live titles through, or `None` when the
    /// source is a local file. Also the playback monitor's test for which kind of source is on the
    /// deck.
    pub fn stream_shared(&self) -> Option<Arc<StreamShared>> {
        self.live_stream.lock().clone()
    }

    fn clear_live_stream(&self) {
        *self.live_stream.lock() = None;
    }

    /// Start the stream staged under `generation`, hard-cutting whatever was playing.
    ///
    /// Always a hard cut: a crossfade ramps between two *tracks*, and there is nothing worth
    /// overlapping a station's first second of buffering with. The `ReplayGain` handed to
    /// `build_source` is unity for the same reason the plan gives — a live stream carries no
    /// per-track tags to bake.
    pub fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError> {
        // Matched before it is taken: a stage belonging to some other session belongs to a *newer*
        // one, and taking it to refuse it would close the connection that session is waiting on.
        let mut staged = self.staged_stream.lock();
        let claimed = staged
            .take_if(|(staged_generation, _)| *staged_generation == generation)
            .map(|(_, prepared)| prepared);
        drop(staged);

        let Some(prepared) = claimed else {
            return Err(AppError::Player("No radio stream is staged for this station".to_owned()));
        };
        let (source, shared) = prepared.into_parts();

        // Published before the deck work rather than after, so the monitor can never see a station
        // playing with no cell to read its buffering state from.
        *self.live_stream.lock() = Some(shared);
        self.bump_epoch();
        let decks = self.lock_decks();
        decks.cut_to(volume, Self::STREAM_SPEED, |deck| {
            self.build_source(source, TrackReplayGain::default(), deck)
        });
        self.crossfade_armed.store(false, Ordering::Release);
        self.gapless_pending.store(false, Ordering::Release);
        Ok(())
    }

    /// Start `file_path` on the idle deck and cross-fade over `fade_ms` media
    /// milliseconds. The outgoing source ends itself when its ramp lands
    /// (`end_on_complete`), draining that deck — which is how
    /// [`Self::is_crossfading`] knows the overlap is over.
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
        // Decode outside the lock, pick the deck and build inside it — see the
        // same note in `play_media`.
        let decoded = decode_file(file_path)?;

        self.bump_epoch();
        self.clear_live_stream();
        let mut decks = self.lock_decks();
        // A staged gapless source sits on the *active* deck and inherits its
        // fade cell, so it would fade out alongside the track it was meant to
        // follow. `crossfade_eligible` gates the preload off for exactly this
        // reason; assert rather than silently mis-fade.
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

    /// Wrap an audio source in what the decks actually play: the graphic EQ, this track's baked
    /// `ReplayGain` and `deck`'s ramp cell, under a visualizer tap writing into `deck`'s own ring.
    ///
    /// Generic over the source rather than over a reader, because a live stream reaches here as a
    /// [`super::prebuffer::PrebufferSource`] and a file as a `Decoder` — the ring sits between the
    /// stream's decoder and the DSP chain, so the two only meet at `Source`. Everything downstream
    /// is identical, which is the point: the EQ, the limiter and the visualizer work on a station
    /// with no code of their own.
    ///
    /// Always called with the deck the source is about to be appended to — see [`super::decks`]
    /// for why the two can't be split. Building the tap also *claims* that ring for the life of
    /// the value and stamps its history away if the deck was idle, both only correct for a source
    /// about to play, so don't build one anywhere it might be held or discarded instead.
    fn build_source<S: Source + Send + 'static>(
        &self,
        input: S,
        baked_rg: TrackReplayGain,
        deck: &Deck,
    ) -> VisualizerTap<EqSource<S>> {
        VisualizerTap::new(
            EqSource::new(input, self.eq.clone(), self.rg.clone(), baked_rg, deck.fade.clone()),
            &self.viz,
            deck.viz_slot,
        )
    }

    /// Whether a crossfade's outgoing deck is still audible. Clearing the flag
    /// here once that deck drains doubles as the crossfade's completion hook —
    /// no `Drop` impl or extra atomic needed. Safe because `Player::append`
    /// bumps `sound_count` synchronously, so a just-fed deck never reports empty.
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

    /// Seek the (already-locked) player to a MEDIA-time position.
    ///
    /// `try_seek` takes output time, which `Speed::try_seek` multiplies back up
    /// to reach the decoder — so passing `media_ms` straight through would land
    /// the decoder on `media_ms × speed` and read back wrong.
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
            // Armed unconditionally, not gated on the setting: a faded pause
            // leaves the deck holding silence and the setting may have been
            // turned off in between. A zero-length ramp snaps back to unity.
            let ramp = if self.xf.fade_on_pause() {
                crossfade::PAUSE_FADE_MS
            } else {
                0
            };
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
    /// the ramp has; `false` means refused and the caller runs the immediate op.
    ///
    /// The ramp holds at zero rather than ending its source — a self-ending fade
    /// would drain the active deck and read as `EndOfStream`.
    fn arm_fade_out(&self, fade_ms: u64, op: DeferredOp) -> bool {
        if !self.can_fade_out(fade_ms) {
            return false;
        }
        let epoch = self.bump_epoch();
        {
            let decks = self.lock_decks();
            // Backstop for the gapless race, mirroring `play_media`: the read in
            // `can_fade_out` is lock-free and the bump is a separate step, so a
            // preload can complete between the two and only this re-read sees
            // it. One arriving *after* the bump is benign — the pause path just
            // holds it at silence until the resume ramps back to unity, and the
            // stop path's deferred `ClearAll` takes source and flag together.
            if self.gapless_pending.load(Ordering::Acquire) {
                return false;
            }
            decks.active().fade.arm(None, 0.0, fade_ms, false);
        }
        self.schedule_after(epoch, fade_ms, op);
        true
    }

    /// Fade to silence over `fade_ms`, then pause both decks.
    pub fn pause_with_fade(&self, fade_ms: u64) {
        if !self.arm_fade_out(fade_ms, DeferredOp::PauseAll) {
            self.pause();
        }
    }

    /// Stop playback. `Player::clear()` removes all sources and pauses automatically.
    pub fn stop(&self) {
        self.bump_epoch();
        self.crossfade_armed.store(false, Ordering::Release);
        // Clearing the decks drops a live stream's source, whose `Drop` is what tells its feed
        // thread to close the connection; this only stops anyone still reading the cell.
        self.clear_live_stream();
        let decks = self.lock_decks();
        decks.clear_all();
        self.gapless_pending.store(false, Ordering::Release);
    }

    /// Fade to silence over `fade_ms`, then clear both decks via the deferred
    /// [`DeferredOp::ClearAll`].
    ///
    /// `gapless_pending` is deliberately *not* cleared here: the gate already
    /// refused the fade if a source was staged, so clearing eagerly could only
    /// start lying about one the deferred clear has yet to remove. A preload
    /// slipping in behind us is dropped with the flag by that `ClearAll`; one
    /// behind *the clear* finds an empty deck and is refused.
    pub fn stop_with_fade(&self, fade_ms: u64) {
        if !self.arm_fade_out(fade_ms, DeferredOp::ClearAll) {
            self.stop();
        }
    }

    pub fn seek(&self, position_ms: u64) {
        self.abort_crossfade();
        // Deliberately does NOT bump the epoch: a seek replaces no deck
        // contents, and cancelling a pending deferred pause would leave the
        // decks running silently at the ramp's zero gain while the UI reads
        // Paused — seeking a paused track is an ordinary thing to do.
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
        let media_ms = compute_position(player.get_pos(), old_speed);
        // Both decks must run at the same speed or a crossfade drifts, but only
        // the active one carries a position worth re-anchoring.
        decks.set_speed_all(speed);
        // The tap sits *under* rodio's speed stage, so the analyzer needs the
        // factor to place band edges on the pitch you hear. Sole writer of that
        // cell — see `VisualizerShared::set_speed`.
        self.viz.set_speed(speed);
        // Re-anchor rodio's position tracker, or `get_pos()` keeps the output
        // time it accumulated at the old speed and `query_position` rescales
        // that whole elapsed portion, jumping the UI position. Skipped at 0 to
        // avoid a spurious decoder seek on boot.
        if media_ms > 0 {
            Self::seek_to_media(player, media_ms, speed);
        }
    }

    /// Enable / disable the graphic equalizer. Lock-free — every `EqSource`
    /// (playing + preloaded) picks the change up on its next sample.
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

    /// Set the EQ preamp / master gain (dB).
    pub fn set_eq_preamp(&self, preamp_db: f32) {
        self.eq.set_preamp(preamp_db);
    }

    /// Enable / disable `ReplayGain`. Lock-free, like the EQ setters.
    pub fn set_replaygain_enabled(&self, enabled: bool) {
        self.rg.set_enabled(enabled);
    }

    pub fn set_replaygain_mode(&self, mode: RgMode) {
        self.rg.set_mode(mode);
    }

    /// Set the `ReplayGain` preamp (dB).
    pub fn set_replaygain_preamp(&self, preamp_db: f32) {
        self.rg.set_preamp(preamp_db);
    }

    /// Enable / disable the static peak-based clip guard.
    pub fn set_replaygain_prevent_clipping(&self, on: bool) {
        self.rg.set_prevent_clipping(on);
    }

    /// Arm / disarm the visualizer's sample tap; while off it never touches the
    /// ring. The UI does not come through here — it already holds the cell via
    /// [`Self::visualizer`] for snapshotting and arms it off the Now-Playing
    /// view's visibility. This spelling exists for the crossfade integration test.
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
    /// rodio's queue plays them back-to-back. Mutually exclusive with a
    /// crossfade for a given transition — see `crossfade::crossfade_eligible`.
    pub fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) {
        let Some(path) = file_path else {
            // The action layer only ever *clears* a stale preload through here.
            self.gapless_pending.store(false, Ordering::Release);
            return;
        };

        // Snapshot before the unlocked decode: a mismatch below means the decks
        // moved out from under this preload, so staging anyway would queue the
        // wrong track *and*, if the active deck flipped, hand it the other
        // deck's ramp cell — possibly one armed to fade out and end.
        let epoch = self.deck_epoch.load(Ordering::Acquire);

        // Decode off the deck lock, as in `play_media` — a preload fires right
        // when a stall in position publication would be most visible.
        let decoded = match decode_file(path) {
            Ok(decoded) => decoded,
            Err(e) => {
                log::warn!("Failed to preload gapless track {path}: {e}");
                self.gapless_pending.store(false, Ordering::Release);
                return;
            }
        };

        // Re-check and stage under one lock. `Deck::stage` takes the builder
        // rather than a source, for the reason the other two appends do.
        let decks = self.lock_decks();
        if self.deck_epoch.load(Ordering::Acquire) != epoch {
            log::debug!("Dropping stale gapless preload of {path}");
            return;
        }
        let deck = decks.active();
        // The epoch can't stand in for this: `stop()` and the deferred
        // [`DeferredOp::ClearAll`] empty the decks *without* bumping it, so a
        // preload whose decode outran either would pass the re-check, append
        // behind nothing, and leave `gapless_pending` set over an emptied deck —
        // which `check_playback_state` reads as a phantom `GaplessTransition`.
        // Never rejects a legitimate stage: the only caller passing a path is
        // the monitor's late preload, from its `Playing` branch, and
        // `play_media` appends under this same lock.
        if deck.player.empty() {
            log::debug!("Dropping gapless preload of {path}: nothing left to follow");
            return;
        }
        deck.stage(|d| self.build_source(decoded, baked_rg, d));
        self.gapless_pending.store(true, Ordering::Release);
    }

    /// Current playback position in milliseconds, on the media timeline.
    pub fn query_position(&self) -> u64 {
        let decks = self.lock_decks();
        let player = &decks.active().player;
        let wall_time = player.get_pos();
        let speed = f64::from(player.speed());
        compute_position(wall_time, speed)
    }

    /// One lock acquisition, so gapless-transition and end-of-stream detection
    /// can't race each other.
    ///
    /// Reads the **active** deck only. During a crossfade that deck holds
    /// exactly the incoming track and this reports `Playing`; the outgoing deck
    /// draining is [`Self::is_crossfading`]'s business.
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

    /// Enable / disable crossfade. Lock-free, like the EQ setters.
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

/// How long the file plays for, as the container reports it, or `None` when it carries
/// no frame count or no decoder is registered for its codec.
///
/// The scan path's answer of last resort. Lofty reads duration off the same parse that
/// reads the tags, so a file it can't identify (a Matroska or CAF one, say) reaches the
/// database with no length at all unless someone asks the decoder instead
/// (`media::metadata`). It costs a probe plus one decoded packet, which is why it stays
/// on that failure path rather than running for every file scanned.
pub fn probe_duration(path: &Path) -> Option<Duration> {
    decode_file(path.to_str()?).ok()?.total_duration()
}

fn decode_file(path: &str) -> Result<Decoder<BufReader<File>>, AppError> {
    let file =
        File::open(path).map_err(|e| AppError::Player(format!("Cannot open {path}: {e}")))?;
    let file_len = file.metadata().map(|m| m.len()).ok();

    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("");

    // Symphonia pulls frames in chunks well above the std 8 KB default for most
    // formats, so a small buffer costs a refill per frame. 64 KB covers typical
    // FLAC/MP3 frame clusters without meaningful per-track memory.
    let mut builder = Decoder::builder()
        .with_data(BufReader::with_capacity(64 * 1024, file))
        .with_hint(ext)
        .with_gapless(true)
        .with_seekable(true);

    if let Some(len) = file_len {
        builder = builder.with_byte_len(len);
    }

    builder.build().map_err(|e| AppError::Player(format!("Decode error for {path}: {e}")))
}

#[cfg(test)]
#[path = "tests/rodio_backend_tests.rs"]
mod tests;
