//! The device's own volume control, carrying the level while an exclusive claim plays the voices at
//! unity: how a claim takes, follows, watches and gives it back, once for both backends. Each
//! supplies a [`VolumeControl`] speaking the percentage the system shows, so both sliders agree.
//!
//! A claim never turns the device up past what the system chose; the slider follows it down
//! instead. A level a release in this run put back is resuming rather than chosen, or every reopen
//! would drag the slider down. The control stays the system's own, so its moves are watched and
//! handed back. A release restores the original, which outlives a claim on a device unplugged
//! mid-claim until a release can put it back.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

use melodia_core::error::describe;

use super::device::Feed;

/// How often a moving volume reaches the device. Each set is a call into the system, made on the
/// thread with a period to refill, so a slider drag mustn't cost one per period.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(20);

/// How often the device is asked whether something else moved it. Quick enough that Melodia's
/// slider follows the system's while it is dragged, and rare enough to cost the writer nothing.
const WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// How far the device's level has to move to count as someone moving it: well under the whole
/// percent the system's slider steps in, and well over any rounding of a level Melodia set.
const MOVE_THRESHOLD: f32 = 0.001;

/// How near the device has to sit to a volume already for setting it to be skipped: under the half
/// percent a followed move comes back rounded to.
const FOLLOWED_TOLERANCE: f32 = 0.005;

/// Each device's level from before Melodia first claimed it, by device id, until a release has put
/// it back.
static ORIGINAL_LEVELS: Mutex<Vec<(String, f32)>> = Mutex::new(Vec::new());

/// The level a release last put each device back to, by device id. A claim finding the device
/// still there is resuming Melodia's own hand-back rather than meeting a level the system chose.
static RESTORED_LEVELS: Mutex<Vec<(String, f32)>> = Mutex::new(Vec::new());

/// A device's own volume control, read and set as the fraction the system's slider shows for it.
pub(super) trait VolumeControl {
    type Error: std::error::Error;

    fn level(&self) -> Result<f32, Self::Error>;

    fn set_level(&self, level: f32) -> Result<(), Self::Error>;
}

/// A claimed device's volume control, put back to its original level when dropped.
pub(super) struct HardwareVolume<C: VolumeControl> {
    control: C,
    id: String,
    /// The volume last set, and when, so an unchanged one costs no call.
    applied: Option<(f64, Instant)>,
    /// The level the device last reported, read back after each set so the device's own rounding
    /// of it never reads as a move.
    reported: f32,
    /// When the device was last asked whether something else moved it.
    watched: Instant,
    /// The level the claim kept the device at, where Melodia's volume was higher.
    lowered: Option<f64>,
}

impl<C: VolumeControl> HardwareVolume<C> {
    /// `control`, the device `id`'s own, set to `volume` unless the system had it quieter.
    pub(super) fn take(control: C, id: &str, volume: f64) -> Result<Self, C::Error> {
        let current = control.level()?;
        remember_original(id, current);
        let resuming = restored_level(id).is_some_and(|restored| already_at(current, restored));
        let now = Instant::now();
        let mut taken = Self {
            control,
            id: id.to_owned(),
            applied: None,
            reported: current,
            watched: now,
            lowered: None,
        };
        if !resuming && would_raise(level_for(volume), current) {
            // Marked applied, or the next `follow` would raise it before the slider comes down.
            taken.applied = Some((volume, now));
            taken.lowered = Some(f64::from(current));
        } else {
            taken.set(volume, now)?;
        }
        Ok(taken)
    }

    /// The level the claim kept the device at rather than raise it to Melodia's, for the slider
    /// to follow down. `None` where the device took Melodia's level.
    pub(super) fn lowered(&self) -> Option<f64> {
        self.lowered
    }

    /// A writer's turn, once per period after its write, where the period's slack is: follow the
    /// slider, then hand back a move made outside Melodia.
    pub(super) fn sync(&mut self, feed: &Feed) -> Result<(), C::Error> {
        self.follow(feed.volume.load())?;
        if let Some(level) = self.take_move()? {
            feed.external_volume.report(level);
        }
        Ok(())
    }

    /// At most once per [`FOLLOW_INTERVAL`], a held-back move landing later. A level the device
    /// already sits at isn't written: a handed-back system move returns rounded, and writing it
    /// would undo a move the system made meanwhile.
    fn follow(&mut self, volume: f64) -> Result<(), C::Error> {
        self.follow_at(volume, Instant::now())
    }

    /// [`Self::follow`] with the clock passed in, so the pacing can be stepped.
    fn follow_at(&mut self, volume: f64, now: Instant) -> Result<(), C::Error> {
        let due = self.applied.is_none_or(|(applied, at)| {
            applied.to_bits() != volume.to_bits() && now.duration_since(at) >= FOLLOW_INTERVAL
        });
        if !due {
            return Ok(());
        }
        if already_at(level_for(volume), self.reported) {
            self.applied = Some((volume, now));
            return Ok(());
        }
        self.set(volume, now)
    }

    /// The level something outside Melodia moved the device to since the last call, looked for at
    /// most once per [`WATCH_INTERVAL`].
    fn take_move(&mut self) -> Result<Option<f64>, C::Error> {
        self.take_move_at(Instant::now())
    }

    /// [`Self::take_move`] with the clock passed in, so the pacing can be stepped.
    fn take_move_at(&mut self, now: Instant) -> Result<Option<f64>, C::Error> {
        if now.duration_since(self.watched) < WATCH_INTERVAL {
            return Ok(None);
        }
        self.watched = now;
        let level = self.control.level()?;
        if (level - self.reported).abs() < MOVE_THRESHOLD {
            return Ok(None);
        }
        self.reported = level;
        Ok(Some(f64::from(level)))
    }

    fn set(&mut self, volume: f64, now: Instant) -> Result<(), C::Error> {
        self.control.set_level(level_for(volume))?;
        self.reported = self.control.level()?;
        self.applied = Some((volume, now));
        Ok(())
    }
}

impl<C: VolumeControl> Drop for HardwareVolume<C> {
    fn drop(&mut self) {
        let mut originals = ORIGINAL_LEVELS.lock();
        let Some(index) = originals.iter().position(|(id, _)| *id == self.id) else { return };
        match self.control.set_level(originals[index].1) {
            Ok(()) => {
                let (_, restored) = originals.swap_remove(index);
                remember_restored(&self.id, restored);
            }
            // Kept for the next claim of the device: one that has gone can't be set either.
            Err(e) => {
                log::debug!("audio: the device's own volume wasn't put back: {}", describe(&e));
            }
        }
    }
}

/// `taken`, or `None` where `device_name`'s own control refused, which is logged: the claim goes
/// ahead either way, with the voices carrying the level.
pub(super) fn or_software<C: VolumeControl>(
    device_name: &str,
    taken: Result<Option<HardwareVolume<C>>, impl std::error::Error>,
) -> Option<HardwareVolume<C>> {
    taken.unwrap_or_else(|e| {
        log::info!(
            "audio: {device_name} keeps the volume in software, its own control refused: {}",
            describe(&e)
        );
        None
    })
}

/// Note `level` as the device's original, unless a claim that couldn't put one back already did.
fn remember_original(id: &str, level: f32) {
    let mut originals = ORIGINAL_LEVELS.lock();
    if !originals.iter().any(|(known, _)| known == id) {
        originals.push((id.to_owned(), level));
    }
}

/// Note `level` as the one a release just put the device `id` back to.
fn remember_restored(id: &str, level: f32) {
    let mut restored = RESTORED_LEVELS.lock();
    match restored.iter_mut().find(|(known, _)| known == id) {
        Some(entry) => entry.1 = level,
        None => restored.push((id.to_owned(), level)),
    }
}

/// The level a release last put the device `id` back to, or `None` where none has in this run.
fn restored_level(id: &str) -> Option<f32> {
    RESTORED_LEVELS.lock().iter().find(|(known, _)| known == id).map(|(_, level)| *level)
}

/// `volume`, the slider's fraction, as the level the system shows as the same percentage.
/// Silence is the device's floor: the voices silence it already, and a rise from it should start
/// low rather than at whatever level it had before.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction held to 0..=1 loses nothing the device can resolve in f32"
)]
fn level_for(volume: f64) -> f32 {
    if volume.is_nan() {
        return 0.0;
    }
    volume.clamp(0.0, 1.0) as f32
}

/// Whether a device reporting `reported` already plays at `level`, as near as a whole percent on
/// the slider can say.
fn already_at(level: f32, reported: f32) -> bool {
    (level - reported).abs() < FOLLOWED_TOLERANCE
}

/// Whether setting a device at `current` to `level` would turn it up by more than the slider's
/// rounding, which a claim never does.
fn would_raise(level: f32, current: f32) -> bool {
    level > current && !already_at(level, current)
}

#[cfg(test)]
#[path = "tests/hardware_volume_tests.rs"]
mod tests;
