//! What the decks answer without changing, one read each under the decks lock, so none races the
//! transport in `mod.rs`.

use std::time::Duration;

use melodia_playback::player::playback::output::voice::Voice;

use super::PlaybackEngine;

/// The active deck's two positions, read together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeckClock {
    /// Where the ear is: what is shown, persisted and reported outward.
    pub heard_ms: u64,
    /// A device's worth ahead of the ear, and what a transition is timed against, since a ramp has
    /// to land on the source's end.
    pub pulled_ms: u64,
}

impl PlaybackEngine {
    /// How far the active deck has been pulled, in milliseconds on the media timeline.
    pub fn query_position(&self) -> u64 {
        millis(self.lock_decks().active().voice.position())
    }

    /// `None` until the deck has acted on everything sent to it, or a track start or seek would
    /// publish the position it replaced.
    pub fn query_settled_clock(&self) -> Option<DeckClock> {
        self.with_settled_voice(|voice| DeckClock {
            heard_ms: millis(voice.heard(self.output.lead.get())),
            pulled_ms: millis(voice.position()),
        })
    }

    /// [`Self::query_position`], settled. `held` for an empty deck, whose cleared clock a second
    /// release would write back; answered rather than refused, a track that ran out under a pause
    /// fade leaving it empty too.
    pub fn query_settled_position(&self, held: u64) -> Option<u64> {
        self.with_settled_voice(
            |voice| {
                if voice.is_empty() { held } else { millis(voice.position()) }
            },
        )
    }

    fn with_settled_voice<T>(&self, read: impl FnOnce(&Voice) -> T) -> Option<T> {
        let decks = self.lock_decks();
        let voice = &decks.active().voice;
        voice.is_settled().then(|| read(voice))
    }

    /// A pause long enough to give the device back leaves the deck holding no source to resume.
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
