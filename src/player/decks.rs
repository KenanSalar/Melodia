//! The two rodio "voices" the player alternates between, and the primitives the
//! controller drives them with.
//!
//! rodio's [`Player`] *sequences* its sources — `append` schedules a source to
//! start when the previous one ends — so two tracks can never overlap on one
//! [`Player`]. Overlapping them needs two, summed by the shared device mixer.
//! An idle deck costs nothing: `Player::new` builds its queue with
//! `keep_alive_if_empty`, so it emits silence and stays attached rather than
//! detaching.
//!
//! Nothing lands on a deck except through the three primitives that take a
//! *builder* rather than a ready-made source — [`Decks::cut_to`],
//! [`Decks::crossfade_to`] and [`Deck::stage`]. Each hands the closure the very
//! deck it is about to append to, so the [`FadeShared`] cell a source carries can
//! only have come from that deck. Reading a deck earlier and appending later
//! would let a concurrent op flip `active` in between and hand the source the
//! *other* deck's ramp — which may be armed to fade out and end itself. The
//! controller reaches all three only through the decks mutex.

use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};

use rodio::mixer::Mixer;
use rodio::{Player, Source};

use super::crossfade::FadeShared;

/// One rodio voice: an independent [`Player`] on the shared device mixer, plus
/// the crossfade ramp cell every source appended to it reads.
pub struct Deck {
    pub player: Player,
    pub fade: Arc<FadeShared>,
}

impl Deck {
    /// A fresh deck, connected to the mixer and paused.
    fn connect(mixer: &Mixer) -> Self {
        let player = Player::connect_new(mixer);
        player.pause();
        Self {
            player,
            fade: FadeShared::idle(),
        }
    }

    /// Apply the transport parameters and start `source` on this deck.
    pub fn start<S>(&self, source: S, volume: f64, speed: f64)
    where
        S: Source + Send + 'static,
    {
        self.player.set_volume(narrow_audio_param(volume));
        self.player.set_speed(narrow_audio_param(speed));
        self.player.append(source);
        self.player.play();
    }

    /// Queue a source *behind* whatever this deck is already playing — the
    /// gapless stage. rodio sequences a `Player`'s sources, so the staged one
    /// starts the instant the current one drains.
    ///
    /// Takes a builder for the same reason [`Decks::crossfade_to`] does: it is
    /// the only way the ramp cell the source carries is guaranteed to be the one
    /// belonging to the deck it lands on.
    pub fn stage<S>(&self, build: impl FnOnce(&Self) -> S)
    where
        S: Source + Send + 'static,
    {
        self.player.append(build(self));
    }

    /// Drop everything on this deck and disarm its ramp.
    pub fn reset(&self) {
        self.player.clear();
        self.fade.reset();
    }

    /// Is this deck actually pulling samples — i.e. is there something on it we
    /// could fade out of?
    ///
    /// A paused deck is still held for control but is never pulled, so a ramp
    /// armed on it can never advance; an empty one has nothing to fade at all.
    pub fn busy(&self) -> bool {
        !self.player.empty() && !self.player.is_paused()
    }
}

/// The two decks and which of them holds the currently-playing track.
pub struct Decks {
    decks: [Deck; 2],
    active: usize,
}

impl Decks {
    /// Both decks connected to `mixer`, deck 0 active.
    pub fn connect(mixer: &Mixer) -> Self {
        Self {
            decks: std::array::from_fn(|_| Deck::connect(mixer)),
            active: 0,
        }
    }

    /// The deck holding the current track.
    pub fn active(&self) -> &Deck {
        &self.decks[self.active]
    }

    /// The deck *not* holding the current track. During a crossfade this is the
    /// outgoing track, still fading out.
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
            deck.player.set_volume(narrow_audio_param(volume));
        }
    }

    /// Both decks must run at the same speed or a crossfade would drift.
    pub fn set_speed_all(&self, speed: f64) {
        for deck in &self.decks {
            deck.player.set_speed(narrow_audio_param(speed));
        }
    }

    /// Hard cut: clear **both** decks — so a crossfade in flight can't leave its
    /// outgoing track playing behind the new one — and start the source on the
    /// (still) active deck.
    ///
    /// Reusing the live active deck matters: rodio only zeroes a `Player`'s
    /// tracked position when `clear()` actually removes a source, so starting on
    /// a long-idle deck would leave `get_pos()` reporting the previous track's
    /// position for a few milliseconds.
    pub fn cut_to<S>(&self, volume: f64, speed: f64, build: impl FnOnce(&Deck) -> S)
    where
        S: Source + Send + 'static,
    {
        self.clear_all();
        let target = self.active();
        let source = build(target);
        target.start(source, volume, speed);
    }

    /// Overlap: start the source on the idle deck ramping up from silence, fade
    /// the active deck out (its source ends itself when the ramp lands, draining
    /// that deck), and hand the incoming deck the `active` role.
    ///
    /// The outgoing ramp starts from *wherever that deck is* (`start: None`) — it
    /// may itself be mid-fade-in — so the two gains still sum to at most unity.
    /// The curve is complementary linear on purpose: rodio's mixer sums its
    /// voices without clamping.
    pub fn crossfade_to<S>(
        &mut self,
        fade_ms: u64,
        volume: f64,
        speed: f64,
        build: impl FnOnce(&Deck) -> S,
    ) where
        S: Source + Send + 'static,
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

/// Work a fade defers until its ramp has landed. Runs on a tokio task guarded by
/// the deck epoch, so any newer control op cancels it.
#[derive(Copy, Clone)]
pub enum DeferredOp {
    /// Pause both decks (a faded pause).
    PauseAll,
    /// Clear both decks (a faded stop).
    ClearAll,
}

/// Poison-recovering lock. Shared by the controller and the deferred pause/stop
/// task, which owns only the `Arc`, not the player.
pub fn lock_decks(decks: &Mutex<Decks>) -> MutexGuard<'_, Decks> {
    decks.lock().unwrap_or_else(|poisoned| {
        log::error!("RodioPlayer decks mutex was poisoned, recovering");
        poisoned.into_inner()
    })
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
