//! What the decks answer without anything about them changing: the two positions the monitor
//! publishes and times against, and whether there is anything on the active deck at all.
//!
//! Each is one read under the decks lock, so none races the transport in `mod.rs`: nothing here
//! bumps the epoch, stages, clears or claims.

use super::PlaybackEngine;

impl PlaybackEngine {
    /// How far the active deck has been pulled, in milliseconds on the media timeline. What a
    /// transition is timed against: a ramp has to land on the source's end, not on the ear's.
    pub fn query_position(&self) -> u64 {
        let position = self.lock_decks().active().voice.position();
        u64::try_from(position.as_millis()).unwrap_or(u64::MAX)
    }

    /// [`Self::query_position`] as the user hears it, with what the device still holds taken off.
    /// What is shown, persisted and reported outward.
    pub fn query_heard_position(&self) -> u64 {
        let position = self.lock_decks().active().voice.heard(self.output.lead.get());
        u64::try_from(position.as_millis()).unwrap_or(u64::MAX)
    }

    /// Whether the active deck holds a source for a resume to carry on with. A pause long enough
    /// to give an exclusive device back leaves it holding none.
    pub fn holds_source(&self) -> bool {
        !self.lock_decks().active().voice.is_empty()
    }
}
