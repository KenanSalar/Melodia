//! The playback engine: the decks, the epoch that orders every op against them, and the transport
//! the action layer drives.
//!
//! One file because everything here races something else here, over the decks lock,
//! [`PlaybackEngine::deck_epoch`] or a station's generation, and those arguments read only side by
//! side. The submodules race nothing: [`controls`], [`reads`], [`player_backend`], and [`output`],
//! which a transport op calls with the decks lock held.

mod controls;
mod output;
mod player_backend;
mod reads;

pub use output::OutputChoice;
pub use player_backend::PlayerBackend;
pub use reads::DeckClock;

use output::EngineOutput;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use melodia_core::error::AppError;
use melodia_core::error::describe;
use parking_lot::Mutex;

use melodia_audio::player::source::audio::AudioSource;
use melodia_audio::player::source::file_decode::FileDecoder;
use melodia_audio::player::source::prebuffer::StreamShared;
use melodia_audio::player::source::stream_source::PreparedStream;
use melodia_playback::player::playback::crossfade::{self, CrossfadeShared};
use melodia_playback::player::playback::decks::{Deck, Decks, DeferredOp, lock_decks};
use melodia_playback::player::playback::equalizer::{self, EqShared, EqSource};
use melodia_playback::player::playback::output::AudioOutput;
use melodia_playback::player::playback::output::convert::Resampler;
use melodia_playback::player::playback::output::mixer::Mixer;
use melodia_playback::player::playback::replaygain::{ReplayGainShared, TrackReplayGain};
use melodia_playback::player::playback::visualizer::{VisualizerShared, VisualizerTap};

/// Result of checking what is on the active deck in a single lock acquisition.
#[derive(Debug, PartialEq)]
pub enum PlaybackCheck {
    /// The staged source took the active voice over.
    GaplessTransition,
    EndOfStream,
    Playing,
}

/// [`PlaybackEngine::check_playback_state`]'s decision, over the active voice's source count.
pub fn evaluate_playback_check(was_gapless: bool, sources: usize) -> PlaybackCheck {
    if was_gapless && sources <= 1 {
        return PlaybackCheck::GaplessTransition;
    }
    if sources == 0 {
        return PlaybackCheck::EndOfStream;
    }
    PlaybackCheck::Playing
}

/// How a track joins the decks.
#[derive(Clone, Copy)]
enum Entry {
    /// On the idle deck, over this many media milliseconds, as the playing track fades out.
    Fade(u64),
    /// Alone, from this far into the track.
    Cut(Duration),
}

pub struct PlaybackEngine {
    // First so it drops first: the stream is what stops the callback.
    output: EngineOutput,
    // The mutex makes a multi-op sequence atomic; `Arc` so a deferred pause/stop can hold the
    // decks after its sleep.
    decks: Arc<std::sync::Mutex<Decks>>,
    // `Arc` because a faded stop's deferred clear drops it with the deck contents.
    gapless_pending: Arc<AtomicBool>,
    // Set while a crossfade's outgoing deck drains; `is_crossfading` self-heals it.
    crossfade_armed: AtomicBool,
    // Bumped by every op that replaces deck contents. A deferred pause/stop and a
    // `preload_gapless` re-check it under the decks lock and bail if it moved.
    deck_epoch: Arc<AtomicU64>,
    // Master state shared by every `EqSource`, so a change reaches the staged track too.
    eq: Arc<EqShared>,
    rg: Arc<ReplayGainShared>,
    // Read by the control layer only, so no generation counter.
    xf: Arc<CrossfadeShared>,
    // One ring per deck, read as their sum; disarmed until Now Playing is on screen.
    viz: Arc<VisualizerShared>,
    // A stream opened for a station generation, waiting for its `PlayStream`. Staged because
    // `PlayerAction` is plain data. Dropping a stage closes its connection, which is how a newer
    // station cancels one still connecting.
    staged_stream: Mutex<Option<(u64, PreparedStream)>>,
    // The playing station's cell, `None` for a file, and the monitor's "is this a station?" test.
    // `Arc` for `gapless_pending`'s reason.
    live_stream: Arc<Mutex<Option<Arc<StreamShared>>>>,
    runtime: tokio::runtime::Handle,
}

impl PlaybackEngine {
    /// The engine over a mixer with no device behind it, which whoever built the mixer pulls.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] if `mixer` carries fewer voices than the decks need.
    pub fn new(mixer: &Mixer, runtime: tokio::runtime::Handle) -> Result<Self, AppError> {
        Ok(Self {
            output: EngineOutput::default(),
            decks: Arc::new(std::sync::Mutex::new(Decks::connect(mixer)?)),
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
        })
    }

    /// The engine playing into `output`, which it keeps so it can reopen it.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] if `output` carries fewer voices than the decks need.
    pub fn with_output(
        output: AudioOutput,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, AppError> {
        let mut engine = Self::new(output.mixer(), runtime)?;
        engine.output = EngineOutput::over(output);
        Ok(engine)
    }

    /// Invalidates any deferred pause/stop and any in-flight gapless preload.
    fn bump_epoch(&self) -> u64 {
        self.deck_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Runs `op` after `delay_ms` unless the epoch moved, re-checked under the decks lock so a
    /// concurrent `play_media` can't be clobbered by a clear that passed a lock-free check.
    fn schedule_after(&self, epoch: u64, delay_ms: u64, op: DeferredOp) {
        let decks = self.decks.clone();
        let deck_epoch = self.deck_epoch.clone();
        let gapless_pending = self.gapless_pending.clone();
        let live_stream = self.live_stream.clone();
        let output = Arc::clone(&self.output.device);
        self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            // `Deck::clear` blocks on the audio callback, twice for `clear_all`, so the landing
            // goes to the blocking pool rather than parking an async worker; sleeping there
            // instead would hold a pool slot for the whole fade.
            tokio::task::spawn_blocking(move || {
                let guard = lock_decks(&decks);
                if deck_epoch.load(Ordering::Acquire) != epoch {
                    return;
                }
                match op {
                    // Keeps the deck contents, staged source included, so leaves the flag alone.
                    DeferredOp::PauseAll => guard.pause_all(),
                    // `stop()`'s deferred half: the flag and the cell drop after the decks, since a
                    // preload behind the epoch bump may have staged onto what this throws away.
                    DeferredOp::ClearAll => {
                        guard.clear_all();
                        gapless_pending.store(false, Ordering::Release);
                        *live_stream.lock() = None;
                        output::release_exclusive(&output);
                    }
                }
            });
        });
    }

    /// See [`Deck::busy`] for why a paused or empty deck can't carry a ramp.
    fn active_deck_busy(&self) -> bool {
        self.lock_decks().active().busy()
    }

    /// The fade gate pause and stop share; a failing term falls through to the immediate op.
    ///
    /// `fade_ms` goes first so `stop_end_of_queue` never takes the deck lock. A stop routinely
    /// lands on a paused deck, a staged gapless source shares the deck's fade cell
    /// ([`crossfade::manual_fade_ms`]), and a crossfade's outgoing ramp has no gain to resume to.
    fn can_fade_out(&self, fade_ms: u64) -> bool {
        fade_ms > 0
            && self.active_deck_busy()
            && !self.is_gapless_preloaded()
            && !self.is_crossfading()
    }

    /// Fade length for a manual track change, `0` for a cut. Only a wish: the gapless read is
    /// advisory and a reopen is unknown until decode, so [`Self::start_track`] settles both under
    /// the deck lock, and whichever of it and the preload commits second yields.
    fn manual_fade_ms(&self, start_position_ms: Option<u64>) -> u64 {
        crossfade::manual_fade_ms(
            self.crossfade_settings(),
            start_position_ms.is_some(),
            self.active_deck_busy(),
            self.is_gapless_preloaded(),
        )
    }

    /// Starts a file already positioned at `start_position_ms`, so the monitor never reads ~0
    /// between the play and the seek.
    pub fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError> {
        let fade_ms = self.manual_fade_ms(start_position_ms);

        // Off the deck lock: the monitor takes it every tick, and a Symphonia probe under it
        // stalls the poll.
        let mut decoded = FileDecoder::open(Path::new(file_path))?;

        // One value for the seek and the clock anchor, so the two can't disagree.
        let mut start = start_position_ms.map_or(Duration::ZERO, Duration::from_millis);

        // Seeked before mounting: through the deck, the demuxer scan would run on the callback.
        if !start.is_zero() {
            log::debug!("Resuming playback at {}ms", start.as_millis());
            if let Err(e) = decoded.try_seek(start) {
                log::warn!("Seek failed: {}", describe(&e));
                // A refused seek leaves the source where it was.
                start = Duration::ZERO;
            }
        }

        // A manual fade is never asked for with a resume position, so the two can't both apply.
        let entry = if fade_ms > 0 { Entry::Fade(fade_ms) } else { Entry::Cut(start) };
        self.start_track(decoded, baked_rg, entry, volume, speed);
        Ok(())
    }

    /// Mounts `decoded` as `entry` asks, cutting from the top where a fade can't go: behind a
    /// gapless source staged since, or onto an output that has to reopen. A fade never reopens,
    /// so whatever a track start would reclaim waits for the next cut.
    fn start_track(
        &self,
        decoded: FileDecoder,
        baked_rg: TrackReplayGain,
        entry: Entry,
        volume: f64,
        speed: f64,
    ) {
        self.bump_epoch();
        self.clear_live_stream();
        let mut decks = self.lock_decks();
        let fade_blocked = self.gapless_pending.load(Ordering::Acquire)
            || !self.plays_without_reopen(decoded.shape(), decoded.format());
        let entry = match entry {
            Entry::Fade(_) if fade_blocked => Entry::Cut(Duration::ZERO),
            entry => entry,
        };
        // The builder is handed the target deck under the append's lock, so the ramp cell is that
        // deck's. `EqSource::new` does no I/O.
        match entry {
            Entry::Fade(fade_ms) => {
                let volume = self.route_volume(volume);
                decks.crossfade_to(fade_ms, volume, speed, |deck| {
                    self.build_source(decoded, baked_rg, deck)
                });
                self.crossfade_armed.store(true, Ordering::Release);
            }
            Entry::Cut(start) => {
                self.cut_to_source(&decks, decoded, baked_rg, volume, speed, start);
            }
        }
        self.gapless_pending.store(false, Ordering::Release);
    }

    /// Plays `source` alone from `start`, reopening the output first. The decks pause ahead of the
    /// reopen, which keeps the voices, or the new stream would play the outgoing source on.
    fn cut_to_source<S: AudioSource + 'static>(
        &self,
        decks: &MutexGuard<'_, Decks>,
        source: S,
        baked_rg: TrackReplayGain,
        volume: f64,
        speed: f64,
        start: Duration,
    ) {
        decks.pause_all();
        let volume = self.prepare_output_for(decks, &source, volume);
        decks.cut_to(volume, speed, start, |deck| self.build_source(source, baked_rg, deck));
        self.crossfade_armed.store(false, Ordering::Release);
    }

    /// Speed while a station plays: anything else consumes a real-time mount faster or slower than
    /// it arrives, and its ring starves or overruns.
    /// [`super::state::PlayerState::build_station_connecting_actions`] pins the state to match.
    const STREAM_SPEED: f64 = 1.0;

    /// Parks an opened live stream for the `PlayStream` action that follows it.
    pub fn stage_stream(&self, generation: u64, prepared: PreparedStream) {
        *self.staged_stream.lock() = Some((generation, prepared));
    }

    /// Drops the stage if it is still `generation`'s, for a session that ended before its open
    /// finished. Otherwise the connection stays open until another station stages over it.
    pub fn discard_staged_stream(&self, generation: u64) {
        let mut staged = self.staged_stream.lock();
        if staged.as_ref().is_some_and(|(staged_generation, _)| *staged_generation == generation) {
            *staged = None;
        }
    }

    /// The playing station's buffering and live-title cell, `None` for a file.
    pub fn stream_shared(&self) -> Option<Arc<StreamShared>> {
        self.live_stream.lock().clone()
    }

    fn clear_live_stream(&self) {
        *self.live_stream.lock() = None;
    }

    /// Starts the stream staged under `generation`, always as a hard cut: nothing is worth
    /// overlapping a station's first second of buffering with, and it has no per-track gain.
    pub fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError> {
        // Matched before it is taken: another session's stage is a newer one's, and taking it
        // would close the connection that session is waiting on.
        let mut staged = self.staged_stream.lock();
        let claimed = staged
            .take_if(|(staged_generation, _)| *staged_generation == generation)
            .map(|(_, prepared)| prepared);
        drop(staged);

        let Some(prepared) = claimed else {
            return Err(AppError::player_msg("No radio stream is staged for this station"));
        };
        let (source, shared) = prepared.into_parts();

        // Published before the deck work, so the monitor never sees a station with no cell.
        *self.live_stream.lock() = Some(shared);
        self.bump_epoch();
        let decks = self.lock_decks();
        // A live mount has no timeline to resume on.
        let start = Duration::ZERO;
        self.cut_to_source(
            &decks,
            source,
            TrackReplayGain::default(),
            volume,
            Self::STREAM_SPEED,
            start,
        );
        self.gapless_pending.store(false, Ordering::Release);
        Ok(())
    }

    /// Starts `file_path` on the idle deck, fading over `fade_ms` media milliseconds; the outgoing
    /// source ends itself when its ramp lands, which is how [`Self::is_crossfading`] sees the
    /// overlap end. A decode failure touches no deck, so the caller can fall back to a skip, and a
    /// track the output must reopen for is cut to, the monitor's decision having gone stale.
    pub fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError> {
        let decoded = FileDecoder::open(Path::new(file_path))?;

        // A staged source would share the outgoing fade cell. `crossfade_eligible` keeps the
        // preload off, so one here is a bug `start_track`'s fallback cut would hide.
        debug_assert!(
            !self.gapless_pending.load(Ordering::Acquire),
            "crossfade must never race a staged gapless preload"
        );
        self.start_track(decoded, baked_rg, Entry::Fade(fade_ms), volume, speed);
        Ok(())
    }

    /// Wraps a source in what the decks play: the EQ, this track's baked `ReplayGain` and `deck`'s
    /// ramp cell, under a visualizer tap on `deck`'s ring. Generic over [`AudioSource`] so a
    /// station's ring and a [`FileDecoder`] share all of it.
    ///
    /// Only for a source about to be appended to `deck`: the tap claims that ring for its life and
    /// stamps an idle deck's history away.
    fn build_source<S: AudioSource + 'static>(
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

    /// Whether a crossfade's outgoing deck is still audible. Clearing the flag once it drains is
    /// the crossfade's completion hook, safe because `Deck::append` counts a source before sending.
    pub fn is_crossfading(&self) -> bool {
        if !self.crossfade_armed.load(Ordering::Acquire) {
            return false;
        }
        let decks = self.lock_decks();
        if decks.idle().voice.is_empty() {
            drop(decks);
            self.crossfade_armed.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// Cancels a crossfade ahead of a seek, which would otherwise rebuild the incoming source and
    /// replay its fade-in from silence. The rebuilt source starts at unity;
    /// [`crossfade::ABORT_RAMP_MS`] is the glide a seek that bails still gets.
    fn abort_crossfade(&self) {
        if !self.crossfade_armed.swap(false, Ordering::AcqRel) {
            return;
        }
        let decks = self.lock_decks();
        decks.idle().reset();
        decks.active().fade.arm(None, 1.0, crossfade::ABORT_RAMP_MS, false);
    }

    pub fn resume(&self) {
        self.bump_epoch();
        // A crossfade owns both ramps; re-arming the active one would restart its fade-in.
        let crossfading = self.is_crossfading();
        let decks = self.lock_decks();
        decks.play_all();
        if !crossfading {
            // Armed whatever the setting: a faded pause left the deck at silence and the setting
            // may have changed since. A zero-length ramp snaps to unity.
            let ramp = if self.xf.fade_on_pause() { crossfade::PAUSE_FADE_MS } else { 0 };
            decks.active().fade.arm(None, 1.0, ramp, false);
        }
    }

    /// Pauses both decks at once; [`Self::pause_with_fade`] is the faded one.
    pub fn pause(&self) {
        self.bump_epoch();
        self.lock_decks().pause_all();
    }

    /// Fades the active deck to silence and schedules `op` for when it lands, `false` if refused.
    /// The ramp holds at zero rather than ending its source, which would read as `EndOfStream`.
    fn arm_fade_out(&self, fade_ms: u64, op: DeferredOp) -> bool {
        if !self.can_fade_out(fade_ms) {
            return false;
        }
        let epoch = self.bump_epoch();
        {
            let decks = self.lock_decks();
            // `can_fade_out`'s read is lock-free, so a preload landing before the bump shows only
            // here. One after it is benign: a pause holds it at silence, and a stop's `ClearAll`
            // takes it with the flag.
            if self.gapless_pending.load(Ordering::Acquire) {
                return false;
            }
            decks.active().fade.arm(None, 0.0, fade_ms, false);
        }
        self.schedule_after(epoch, fade_ms, op);
        true
    }

    pub fn pause_with_fade(&self, fade_ms: u64) {
        if !self.arm_fade_out(fade_ms, DeferredOp::PauseAll) {
            self.pause();
        }
    }

    pub fn stop(&self) {
        self.bump_epoch();
        self.crossfade_armed.store(false, Ordering::Release);
        // Dropping the source is what closes a stream's connection; this only stops readers.
        self.clear_live_stream();
        let decks = self.lock_decks();
        decks.clear_all();
        self.gapless_pending.store(false, Ordering::Release);
        output::release_exclusive(&self.output.device);
    }

    /// Fades to silence, then clears both decks through [`DeferredOp::ClearAll`].
    ///
    /// Leaves `gapless_pending` alone: the gate refused the fade if a source was staged, and one
    /// slipping in behind is dropped with the flag by that `ClearAll`.
    pub fn stop_with_fade(&self, fade_ms: u64) {
        if !self.arm_fade_out(fade_ms, DeferredOp::ClearAll) {
            self.stop();
        }
    }

    /// Moves the playing track to `position_ms` by handing the deck a source already there, so the
    /// demuxer scan runs on the caller's thread rather than in the audio callback; hence
    /// `file_path` and `baked_rg` on the action. A container with no seek index is walked from the
    /// top.
    ///
    /// The deck is claimed before the open and checked after it: a gapless successor can take it
    /// over meanwhile, in the callback, under neither `exec_lock` nor the epoch. Both reads name
    /// the same voice because only `Decks::crossfade_to` moves `active`, under that `exec_lock`.
    ///
    /// Bumps no epoch: a seek replaces no deck contents, and cancelling a deferred pause would
    /// leave the decks running at the ramp's zero gain while the UI reads Paused.
    pub fn seek(&self, file_path: &str, position_ms: u64, baked_rg: TrackReplayGain) {
        self.abort_crossfade();

        // An empty deck, or one a successor has taken (so `file_path` just ended), reads as other
        // than `Playing`; asked before the open so neither costs I/O.
        let handovers = {
            let decks = self.lock_decks();
            let voice = &decks.active().voice;
            let gapless = self.gapless_pending.load(Ordering::Acquire);
            if evaluate_playback_check(gapless, voice.len()) != PlaybackCheck::Playing {
                return;
            }
            voice.handovers()
        };

        let position = Duration::from_millis(position_ms);
        let mut decoded = match FileDecoder::open(Path::new(file_path)) {
            Ok(decoded) => decoded,
            Err(e) => {
                log::warn!("Seek could not reopen the track: {}", describe(&e));
                return;
            }
        };
        if let Err(e) = decoded.try_seek(position) {
            log::warn!("Seek failed: {}", describe(&e));
            return;
        }

        let decks = self.lock_decks();
        let deck = decks.active();
        deck.voice.replace(self.build_source(decoded, baked_rg, deck), position, handovers);
    }

    /// Sets both decks' speed, or a crossfade would drift. No re-anchoring seek: a deck counts
    /// media frames, which no later change to the ratio can rescale.
    pub fn set_speed(&self, speed: f64) {
        self.lock_decks().set_speed_all(speed);
        // The tap sits above the converter, so the analyzer needs the factor
        // (`VisualizerShared::set_speed`).
        self.viz.set_speed(speed);
    }

    /// Converts with `resampler` from the next block on, the converters carrying their position
    /// across; waits out a reopen in flight for the decks lock.
    pub fn set_resampler(&self, resampler: Resampler) {
        self.lock_decks().set_resampler_all(resampler);
    }

    /// Whether a gapless source is staged behind the playing one.
    pub fn is_gapless_preloaded(&self) -> bool {
        self.gapless_pending.load(Ordering::Acquire)
    }

    /// Stages the next track behind the current one on the active deck. Exclusive with a
    /// crossfade per transition (`crossfade::crossfade_eligible`).
    pub fn preload_gapless(&self, path: &str, baked_rg: TrackReplayGain) {
        // Snapshot before the unlocked decode: if the decks move meanwhile, staging would queue
        // the wrong track and, on a flipped deck, under the other deck's ramp cell.
        let epoch = self.deck_epoch.load(Ordering::Acquire);

        let decoded = match self.open_for_gapless(path) {
            Ok(Some(decoded)) => decoded,
            Ok(None) => return,
            Err(e) => {
                log::warn!("Failed to preload gapless track {path}: {}", describe(&e));
                self.gapless_pending.store(false, Ordering::Release);
                return;
            }
        };

        let decks = self.lock_decks();
        if self.deck_epoch.load(Ordering::Acquire) != epoch {
            log::debug!("Dropping stale gapless preload of {path}");
            return;
        }
        let deck = decks.active();
        // `stop()` and the deferred `ClearAll` empty the decks without bumping the epoch, and a
        // stage behind nothing leaves the flag set, read as a phantom `GaplessTransition`. Refuses
        // no real stage: only the monitor's late preload calls this, from `Playing`.
        if deck.voice.is_empty() {
            log::debug!("Dropping gapless preload of {path}: nothing left to follow");
            return;
        }
        deck.stage(|d| self.build_source(decoded, baked_rg, d));
        self.gapless_pending.store(true, Ordering::Release);
    }

    /// Frees the sources the audio callback has finished with, on the monitor's tick: only a track
    /// change produces one, so a timer of its own would poll an empty queue.
    pub fn collect_spent(&self) {
        let decks = self.lock_decks();
        decks.active().voice.collect_spent();
        decks.idle().voice.collect_spent();
    }

    /// Tells a gapless handover from the end of the stream in one lock acquisition, so the two
    /// can't race. Reads the active deck only, the outgoing one of a crossfade being
    /// [`Self::is_crossfading`]'s. A handover stays unclaimed until the advance takes it, so a
    /// seek in between still sees a deck the state is behind.
    pub fn check_playback_state(&self) -> PlaybackCheck {
        let was_gapless = self.gapless_pending.load(Ordering::Acquire);
        let sources = self.lock_decks().active().voice.len();
        evaluate_playback_check(was_gapless, sources)
    }

    /// Takes the staged successor's handover for the advance, `false` where a track start or a
    /// stop has cleared it since [`Self::check_playback_state`] saw it.
    pub fn claim_gapless_handover(&self) -> bool {
        self.gapless_pending.swap(false, Ordering::AcqRel)
    }

    /// Recovers from poison rather than panicking.
    fn lock_decks(&self) -> MutexGuard<'_, Decks> {
        lock_decks(&self.decks)
    }
}

#[cfg(test)]
#[path = "tests/backend_tests.rs"]
mod tests;
