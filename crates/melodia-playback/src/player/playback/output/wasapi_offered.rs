//! The rates each endpoint offered a claim, remembered for the process's life.
//!
//! Asking which rates an endpoint offers means asking every rate in every layout a claim could use,
//! and a rate it lacks is only known once all of them have been refused. On an output set up for
//! surround that is several times the stereo sweep, long enough to hear as a stall before every
//! claim. An endpoint's rates don't move while it stays in the same layout, so the first claim asks
//! and the rest read the answer.
//!
//! **Where they do move anyway, the answer stays stale until Melodia restarts.** A driver setting
//! changed mid-session can do it without touching the layout. A rate the device gained is then
//! never asked for and plays converted; a rate it lost is asked for and refused before the claim
//! moves on to another.

use std::time::Instant;

use parking_lot::Mutex;

use super::rates::RateSet;

/// Answers kept before the oldest is dropped. There is one per endpoint, speaker layout and source
/// channel count seen, so a session comes nowhere near it.
const CAPACITY: usize = 16;

/// What one sweep asked with: the endpoint, and the channel counts it walks, from the source's up to
/// the device's own. A change in Speaker Setup moves the second and asks again.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ProbeKey {
    pub(super) device: String,
    pub(super) from_channels: u16,
    pub(super) device_channels: u16,
}

static REMEMBERED: Answers = Answers::new();

/// The rates `key`'s sweep found, running `probe` for them the first time they are asked for.
pub(super) fn remembered<E>(
    key: ProbeKey,
    probe: impl FnOnce() -> Result<RateSet, E>,
) -> Result<RateSet, E> {
    REMEMBERED.answer(key, probe)
}

/// Each sweep's answer under the key it asked with, oldest first. A type of its own so a test can
/// hold a set that nothing else in the binary writes to.
struct Answers(Mutex<Vec<(ProbeKey, RateSet)>>);

impl Answers {
    const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    /// `key`'s answer, running `probe` for it where none is kept.
    ///
    /// A sweep that fails is not kept: one cut short by the device being taken or unplugged would
    /// otherwise stand for the rest of the session, short of rates the device has.
    ///
    /// The lock isn't held across `probe`: claims come one at a time, through the output's own
    /// lock, and a sweep can take the better part of a second.
    fn answer<E>(
        &self,
        key: ProbeKey,
        probe: impl FnOnce() -> Result<RateSet, E>,
    ) -> Result<RateSet, E> {
        let known =
            self.0.lock().iter().find(|(asked, _)| *asked == key).map(|&(_, offered)| offered);
        if let Some(offered) = known {
            return Ok(offered);
        }
        let started = Instant::now();
        let offered = probe()?;
        log::debug!(
            "audio: asked {} for its rates in {:?}: {offered:?}",
            key.device,
            started.elapsed()
        );
        let mut answers = self.0.lock();
        if answers.len() == CAPACITY {
            answers.remove(0);
        }
        answers.push((key, offered));
        Ok(offered)
    }
}

#[cfg(test)]
#[path = "tests/wasapi_offered_tests.rs"]
mod tests;
