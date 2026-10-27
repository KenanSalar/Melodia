//! The two voices the player alternates between. A voice sequences its sources, so overlap needs
//! two summed by the mixer; both live for the mixer's life, an idle one costing nothing.
//!
//! Nothing lands on a deck except through the three primitives taking a builder ([`Decks::cut_to`],
//! [`Decks::crossfade_to`], [`Deck::stage`]), so a source's [`FadeShared`] cell and ring slot can
//! only be the deck's it lands on (`.claude/rules/audio-stack.md`).

use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use melodia_core::error::AppError;

use super::crossfade::FadeShared;
use super::output::convert::Resampler;
use super::output::mixer::Mixer;
use super::output::voice::Voice;
use melodia_audio::player::source::audio::AudioSource;

/// The visualizer keeps one ring per deck, so the two counts agree.
pub const DECK_COUNT: usize = 2;

pub struct Deck {
    pub voice: Arc<Voice>,
    pub fade: Arc<FadeShared>,
    pub viz_slot: usize,
}

impl Deck {
    /// `start` anchors the clock, which the deck can't see for itself: a resume hands over a source
    /// already seeked, which mounted at zero would report minutes-deep audio as position zero.
    pub fn start<S>(&self, source: S, volume: f64, speed: f64, start: Duration)
    where
        S: AudioSource + 'static,
    {
        self.voice.set_volume(volume);
        self.voice.set_speed(speed);
        self.voice.append_at(source, start);
        self.voice.play();
    }

    /// The gapless stage, starting the instant the current source drains.
    pub fn stage<S>(&self, build: impl FnOnce(&Self) -> S)
    where
        S: AudioSource + 'static,
    {
        self.voice.stage(build(self));
    }

    pub fn reset(&self) {
        self.voice.clear();
        self.fade.reset();
    }

    /// Whether there is something to fade out of: a paused deck is never pulled, so a ramp on it
    /// can't advance, and an empty one has nothing to fade.
    pub fn busy(&self) -> bool {
        !self.voice.is_empty() && !self.voice.is_paused()
    }
}

/// The two decks and which of them holds the currently-playing track.
pub struct Decks {
    decks: [Deck; DECK_COUNT],
    active: usize,
}

impl Decks {
    /// Both decks taken from `mixer`, deck 0 active and both paused.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] if the mixer was built with fewer voices than [`DECK_COUNT`], which is
    /// a boot-time wiring mistake rather than anything a user can reach.
    pub fn connect(mixer: &Mixer) -> Result<Self, AppError> {
        let mut decks = Vec::with_capacity(DECK_COUNT);
        for slot in 0..DECK_COUNT {
            let voice = mixer.voice(slot).ok_or_else(|| {
                AppError::player_msg(format!("The mixer has no voice {slot} for a deck"))
            })?;
            voice.pause();
            decks.push(Deck { voice, fade: FadeShared::idle(), viz_slot: slot });
        }
        let decks = <[Deck; DECK_COUNT]>::try_from(decks)
            .map_err(|_| AppError::player_msg("Wrong number of decks built"))?;
        Ok(Self { decks, active: 0 })
    }

    /// The deck holding the current track.
    pub fn active(&self) -> &Deck {
        &self.decks[self.active]
    }

    /// The deck *not* holding the current track. During a crossfade this is the outgoing track,
    /// still fading out.
    pub fn idle(&self) -> &Deck {
        &self.decks[1 - self.active]
    }

    /// Clear both decks and disarm both ramps.
    pub fn clear_all(&self) {
        for deck in &self.decks {
            deck.reset();
        }
    }

    pub fn pause_all(&self) {
        for deck in &self.decks {
            deck.voice.pause();
        }
    }

    pub fn play_all(&self) {
        for deck in &self.decks {
            deck.voice.play();
        }
    }

    pub fn set_volume_all(&self, volume: f64) {
        for deck in &self.decks {
            deck.voice.set_volume(volume);
        }
    }

    /// Both decks must run at the same speed or a crossfade would drift.
    pub fn set_speed_all(&self, speed: f64) {
        for deck in &self.decks {
            deck.voice.set_speed(speed);
        }
    }

    pub fn set_resampler_all(&self, resampler: Resampler) {
        for deck in &self.decks {
            deck.voice.set_resampler(resampler);
        }
    }

    /// Clears both decks, so a crossfade in flight can't leave its outgoing track playing behind.
    pub fn cut_to<S>(
        &self,
        volume: f64,
        speed: f64,
        start: Duration,
        build: impl FnOnce(&Deck) -> S,
    ) where
        S: AudioSource + 'static,
    {
        self.clear_all();
        let target = self.active();
        let source = build(target);
        target.start(source, volume, speed, start);
    }

    /// Starts the source on the idle deck from silence and fades the active one out from wherever
    /// it is, which may be mid-fade-in, so the two gains still sum to at most unity.
    pub fn crossfade_to<S>(
        &mut self,
        fade_ms: u64,
        volume: f64,
        speed: f64,
        build: impl FnOnce(&Deck) -> S,
    ) where
        S: AudioSource + 'static,
    {
        let target = 1 - self.active;
        // The target deck may still hold a previous crossfade's outgoing track.
        self.decks[target].voice.clear();
        let source = build(&self.decks[target]);
        self.decks[target].fade.arm(Some(0.0), 1.0, fade_ms, false);
        // Both ramps armed before the incoming deck starts, the sum being unclamped: a callback
        // between them would otherwise overshoot by however long the control thread was preempted.
        // This way round the race costs a dip.
        self.active().fade.arm(None, 0.0, fade_ms, true);
        // From the top: a resume at a position hard-cuts instead.
        self.decks[target].start(source, volume, speed, Duration::ZERO);
        self.active = target;
    }
}

/// Work a fade defers until its ramp has landed. Runs on a tokio task guarded by the deck epoch,
/// so any newer control op cancels it.
#[derive(Copy, Clone)]
pub enum DeferredOp {
    /// Pause both decks (a faded pause).
    PauseAll,
    /// Clear both decks (a faded stop).
    ClearAll,
}

/// Poison-recovering lock. Shared by the controller and the deferred pause/stop task, which owns
/// only the `Arc`, not the player.
pub fn lock_decks(decks: &Mutex<Decks>) -> MutexGuard<'_, Decks> {
    decks.lock().unwrap_or_else(|poisoned| {
        log::error!("PlaybackEngine decks mutex was poisoned, recovering");
        poisoned.into_inner()
    })
}
