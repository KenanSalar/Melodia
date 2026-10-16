//! What the decks answer without anything about them changing: the clock the monitor publishes
//! and times against, the position it releases from, and whether there is anything on the active
//! deck at all.
//!
//! Each is one read under the decks lock, so none races the transport in `mod.rs`: nothing here
//! bumps the epoch, stages, clears or claims.

use std::time::Duration;

use melodia_playback::player::playback::output::voice::Voice;

use super::PlaybackEngine;

/// The active deck's two positions, read together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeckClock {
    /// Where the ear is: what is shown, persisted and reported outward.
    pub heard_ms: u64,
    /// Where the deck has been pulled to, a device's worth ahead of the ear. What a transition is
    /// timed against, since a ramp has to land on the source's end rather than the ear's.
    pub pulled_ms: u64,
}

impl PlaybackEngine {
    /// How far the active deck has been pulled, in milliseconds on the media timeline.
    pub fn query_position(&self) -> u64 {
        millis(self.lock_decks().active().voice.position())
    }

    /// The active deck's clock, or `None` while the deck has yet to act on everything sent to it:
    /// until then it still counts whatever a track start or a seek is replacing, and a reading
    /// published from it puts the old position back on screen.
    pub fn query_settled_clock(&self) -> Option<DeckClock> {
        self.with_settled_voice(|voice| DeckClock {
            heard_ms: millis(voice.heard(self.output.lead.get())),
            pulled_ms: millis(voice.position()),
        })
    }

    /// [`Self::query_position`] once the active deck has acted on everything sent to it, or `None`
    /// while it hasn't, a seek's swap being sent before the deck takes it.
    ///
    /// `held` for a deck holding nothing: a clear zeroes the clock, which a second release would
    /// otherwise write back as where to resume. Answered rather than refused, since a track that ran
    /// out under a pause fade leaves the deck empty too, still holding the device.
    pub fn query_settled_position(&self, held: u64) -> Option<u64> {
        self.with_settled_voice(
            |voice| {
                if voice.is_empty() { held } else { millis(voice.position()) }
            },
        )
    }

    /// `read` over the active voice, under one decks lock, once it has acted on everything sent to
    /// it.
    fn with_settled_voice<T>(&self, read: impl FnOnce(&Voice) -> T) -> Option<T> {
        let decks = self.lock_decks();
        let voice = &decks.active().voice;
        voice.is_settled().then(|| read(voice))
    }

    /// Whether the active deck holds a source for a resume to carry on with. A pause long enough
    /// to give an exclusive device back leaves it holding none.
    pub fn holds_source(&self) -> bool {
        !self.lock_decks().active().voice.is_empty()
    }
}

fn millis(position: Duration) -> u64 {
    u64::try_from(position.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "tests/reads_tests.rs"]
mod tests;
