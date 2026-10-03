//! What the decks answer without anything about them changing: the positions the monitor
//! publishes, times against and releases from, and whether there is anything on the active deck
//! at all.
//!
//! Each is one read under the decks lock, so none races the transport in `mod.rs`: nothing here
//! bumps the epoch, stages, clears or claims.

use std::time::Duration;

use super::PlaybackEngine;

impl PlaybackEngine {
    /// How far the active deck has been pulled, in milliseconds on the media timeline. What a
    /// transition is timed against: a ramp has to land on the source's end, not on the ear's.
    pub fn query_position(&self) -> u64 {
        millis(self.lock_decks().active().voice.position())
    }

    /// [`Self::query_position`] as the user hears it, with what the device still holds taken off.
    /// What is shown, persisted and reported outward.
    pub fn query_heard_position(&self) -> u64 {
        millis(self.lock_decks().active().voice.heard(self.output.lead.get()))
    }

    /// [`Self::query_position`] once the active deck has acted on everything sent to it, or `None`
    /// while it hasn't, a seek's swap being sent before the deck takes it.
    ///
    /// Also `None` for a deck holding nothing: a clear zeroes the clock, and that zero is no
    /// source's position. A release that already took the track off would otherwise write it back
    /// as where to resume.
    pub fn query_settled_position(&self) -> Option<u64> {
        let decks = self.lock_decks();
        let voice = &decks.active().voice;
        (voice.is_settled() && !voice.is_empty()).then(|| millis(voice.position()))
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
