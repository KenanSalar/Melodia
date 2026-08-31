//! The two voices the player alternates between, and the primitives the controller drives them
//! with.
//!
//! A voice *sequences* its sources, so two tracks can never overlap on one — hence two, summed by
//! the shared mixer. Both exist for the life of the mixer: an idle one contributes nothing to the
//! block and costs nothing to leave there, which is what the deck design rests on and what rodio
//! could only approximate with a queue kept alive by feeding it silence.
//!
//! Nothing lands on a deck except through the three primitives taking a *builder* rather than a
//! ready-made source — [`Decks::cut_to`], [`Decks::crossfade_to`] and [`Deck::stage`]. Each hands
//! the closure the very deck it is about to append to, so a source's [`FadeShared`] cell and its
//! visualizer ring slot can only have come from that deck; `src/player/CLAUDE.md` argues why
//! reading a deck earlier and appending later is a bug.

use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};

use crate::error::AppError;

use super::audio::AudioSource;
use super::crossfade::FadeShared;
use super::output::deck::Deck as Voice;
use super::output::mixer::Mixer;

/// How many voices the player alternates between. The visualizer keeps one sample ring per deck,
/// so the two counts have to agree.
pub const DECK_COUNT: usize = 2;

/// One voice on the shared mixer, plus the crossfade ramp cell every source appended to it reads.
pub struct Deck {
    pub player: Arc<Voice>,
    pub fade: Arc<FadeShared>,
    /// Which of the visualizer's rings sources on this deck write into.
    pub viz_slot: usize,
}

impl Deck {
    /// Apply the transport parameters and start `source` on this deck.
    pub fn start<S>(&self, source: S, volume: f64, speed: f64)
    where
        S: AudioSource + 'static,
    {
        self.player.set_volume(volume);
        self.player.set_speed(speed);
        self.player.append(source);
        self.player.play();
    }

    /// Queue a source *behind* whatever this deck is already playing — the gapless stage. A voice
    /// sequences its sources, so the staged one starts the instant the current one drains.
    ///
    /// Takes a builder for the same reason [`Decks::crossfade_to`] does: it is the only way the
    /// ramp cell the source carries is guaranteed to belong to the deck it lands on.
    pub fn stage<S>(&self, build: impl FnOnce(&Self) -> S)
    where
        S: AudioSource + 'static,
    {
        self.player.append(build(self));
    }

    /// Drop everything on this deck and disarm its ramp.
    pub fn reset(&self) {
        self.player.clear();
        self.fade.reset();
    }

    /// Is this deck actually pulling samples — is there something on it to fade out of?
    ///
    /// A paused deck is still held for control but is never pulled, so a ramp armed on it can
    /// never advance; an empty one has nothing to fade at all.
    pub fn busy(&self) -> bool {
        !self.player.is_empty() && !self.player.is_paused()
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
            let player = mixer.deck(slot).ok_or_else(|| {
                AppError::Player(format!("The mixer has no voice {slot} for a deck"))
            })?;
            player.pause();
            decks.push(Deck {
                player,
                fade: FadeShared::idle(),
                viz_slot: slot,
            });
        }
        let decks = <[Deck; DECK_COUNT]>::try_from(decks)
            .map_err(|_| AppError::Player("Wrong number of decks built".to_owned()))?;
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
            deck.player.pause();
        }
    }

    pub fn play_all(&self) {
        for deck in &self.decks {
            deck.player.play();
        }
    }

    pub fn set_volume_all(&self, volume: f64) {
        for deck in &self.decks {
            deck.player.set_volume(volume);
        }
    }

    /// Both decks must run at the same speed or a crossfade would drift.
    pub fn set_speed_all(&self, speed: f64) {
        for deck in &self.decks {
            deck.player.set_speed(speed);
        }
    }

    /// Hard cut: clear **both** decks — so a crossfade in flight can't leave its outgoing track
    /// playing behind the new one — and start the source on the (still) active deck.
    pub fn cut_to<S>(&self, volume: f64, speed: f64, build: impl FnOnce(&Deck) -> S)
    where
        S: AudioSource + 'static,
    {
        self.clear_all();
        let target = self.active();
        let source = build(target);
        target.start(source, volume, speed);
    }

    /// Overlap: start the source on the idle deck ramping up from silence, fade the active deck
    /// out (its source ends itself when the ramp lands, draining that deck), and hand the incoming
    /// deck the `active` role.
    ///
    /// The outgoing ramp starts from *wherever that deck is* (`start: None`) — it may itself be
    /// mid-fade-in — so the two gains still sum to at most unity.
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
        self.decks[target].player.clear();
        let source = build(&self.decks[target]);
        self.decks[target].fade.arm(Some(0.0), 1.0, fade_ms, false);
        self.decks[target].start(source, volume, speed);
        self.active().fade.arm(None, 0.0, fade_ms, true);
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
