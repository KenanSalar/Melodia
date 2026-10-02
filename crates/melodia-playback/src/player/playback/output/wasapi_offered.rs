//! The rates each endpoint offered a claim, remembered for the process's life.
//!
//! Asking which rates an endpoint offers means asking every rate in every layout a claim could use,
//! and a rate it lacks is only known once all of them have been refused. On an output set up for
//! surround that is several times the stereo sweep, long enough to hear as a stall before every
//! claim. An endpoint's rates don't move while it stays in the same layout, so the first claim asks
//! and the rest read the answer.

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

static REMEMBERED: Mutex<Vec<(ProbeKey, RateSet)>> = Mutex::new(Vec::new());

/// The rates `key`'s sweep found, running `probe` for them the first time they are asked for.
///
/// The lock isn't held across `probe`: claims come one at a time, through the output's own lock,
/// and a sweep can take the better part of a second.
pub(super) fn remembered(key: ProbeKey, probe: impl FnOnce() -> RateSet) -> RateSet {
    let known = REMEMBERED.lock().iter().find(|(asked, _)| *asked == key).map(|&(_, offered)| offered);
    if let Some(offered) = known {
        return offered;
    }
    let started = Instant::now();
    let offered = probe();
    log::debug!("audio: asked {} for its rates in {:?}: {offered:?}", key.device, started.elapsed());
    let mut remembered = REMEMBERED.lock();
    if remembered.len() == CAPACITY {
        remembered.remove(0);
    }
    remembered.push((key, offered));
    offered
}
