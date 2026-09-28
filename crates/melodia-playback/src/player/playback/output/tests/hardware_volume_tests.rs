//! Tests for the hardware volume's rules: which level a slider fraction asks the device for, when a
//! claim may turn the device up, when a move is the system's, and what a release puts back.
//!
//! The device is a fake whose level lives in cells the test shares, so a claim, its release and the
//! next claim can meet the same device. The two level memories are process-wide by design, since
//! they outlive every claim, so each test claims a device id of its own.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::{
    FOLLOW_INTERVAL, HardwareVolume, VolumeControl, WATCH_INTERVAL, already_at, level_for,
};

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the device refused the level")
    }
}

impl std::error::Error for Refused {}

/// A device's own control, rounding a level to `steps` the way a mixer does where `steps` is set,
/// and refusing every level while `refuses` is, the way an unplugged one does.
#[derive(Clone, Default)]
struct FakeDevice {
    level: Rc<Cell<f32>>,
    steps: Option<f32>,
    refuses: Rc<Cell<bool>>,
    sets: Rc<Cell<u32>>,
}

impl FakeDevice {
    fn at(level: f32) -> Self {
        let device = Self::default();
        device.level.set(level);
        device
    }

    fn with_steps(level: f32, steps: f32) -> Self {
        Self { steps: Some(steps), ..Self::at(level) }
    }

    /// The system's own slider moving the device.
    fn moved_to(&self, level: f32) {
        self.level.set(level);
    }
}

impl VolumeControl for FakeDevice {
    type Error = Refused;

    fn level(&self) -> Result<f32, Refused> {
        Ok(self.level.get())
    }

    fn set_level(&self, level: f32) -> Result<(), Refused> {
        if self.refuses.get() {
            return Err(Refused);
        }
        self.sets.set(self.sets.get() + 1);
        self.level.set(self.steps.map_or(level, |steps| (level * steps).round() / steps));
        Ok(())
    }
}

/// The fraction goes over as it is, so the percentage the system shows for the device is the one
/// Melodia's slider shows. Anything outside `0..=1` is held to it, since the device refuses a level
/// past its ends rather than taking the nearest.
#[test]
fn the_slider_fraction_is_the_level_the_system_shows() {
    let rows = [
        (0.5, 0.5),
        (0.39, 0.39),
        (1.0, 1.0),
        (0.0, 0.0),
        (-0.25, 0.0),
        (1.5, 1.0),
        (f64::NAN, 0.0),
    ];
    for (volume, expected) in rows {
        let level: f32 = level_for(volume);

        assert_eq!(level.to_bits(), f32::to_bits(expected), "{volume}");
    }
}

/// A system move comes back as the slider's whole-percent rounding of it, and writing that back
/// would undo a move made meanwhile, so inside half a percent the device is already there. A whole
/// step of Melodia's own slider must never count, or the slider would stop moving the device.
#[test]
fn only_the_sliders_own_rounding_counts_as_the_device_already_being_there() {
    let rows = [
        ("the level itself", 0.37, 0.37, true),
        ("rounded down from a move", 0.37, 0.3749, true),
        ("rounded up from a move", 0.38, 0.3751, true),
        ("at the floor", 0.0, 0.004, true),
        ("just past half a percent above", 0.37, 0.3751, false),
        ("just past half a percent below", 0.38, 0.3749, false),
        ("a whole step", 0.38, 0.37, false),
    ];
    for (what, level, reported, expected) in rows {
        assert_eq!(already_at(level, reported), expected, "{what}");
    }
}

/// A slider left at full in shared mode would otherwise blast a device the system keeps low the
/// moment the claim takes its control. The slider follows the device down instead.
#[test]
fn a_claim_never_turns_the_device_up_past_the_systems_level() -> Result<(), Refused> {
    let device = FakeDevice::at(0.4);

    let claim = HardwareVolume::take(device.clone(), "never-raise", 0.8)?;

    assert_eq!(device.sets.get(), 0, "the device was written");
    assert_eq!(device.level.get().to_bits(), 0.4_f32.to_bits());
    assert_eq!(claim.lowered(), Some(f64::from(0.4_f32)));
    Ok(())
}

#[test]
fn a_claim_quieter_than_the_system_takes_melodias_level() -> Result<(), Refused> {
    let device = FakeDevice::at(0.7);

    let claim = HardwareVolume::take(device.clone(), "quieter", 0.3)?;

    assert_eq!(device.level.get().to_bits(), 0.3_f32.to_bits());
    assert_eq!(claim.lowered(), None);
    Ok(())
}

/// Every reopen releases first, so capping a device still at the level the release put back would
/// drag the slider down at each rate change. That device is resuming, and takes Melodia's level.
#[test]
fn a_device_still_at_the_level_a_release_put_back_resumes_melodias() -> Result<(), Refused> {
    let device = FakeDevice::at(0.5);
    drop(HardwareVolume::take(device.clone(), "resume", 0.3)?);
    assert_eq!(device.level.get().to_bits(), 0.5_f32.to_bits(), "the release put nothing back");

    let claim = HardwareVolume::take(device.clone(), "resume", 0.9)?;

    assert_eq!(device.level.get().to_bits(), 0.9_f32.to_bits());
    assert_eq!(claim.lowered(), None);
    Ok(())
}

/// Resuming is only for the level the release left. One the system chose after it caps the next
/// claim like any other.
#[test]
fn a_level_the_system_chose_after_a_release_caps_the_next_claim() -> Result<(), Refused> {
    let device = FakeDevice::at(0.5);
    drop(HardwareVolume::take(device.clone(), "moved-after-release", 0.3)?);
    device.moved_to(0.45);

    let claim = HardwareVolume::take(device.clone(), "moved-after-release", 0.9)?;

    assert_eq!(device.level.get().to_bits(), 0.45_f32.to_bits());
    assert_eq!(claim.lowered(), Some(f64::from(0.45_f32)));
    Ok(())
}

/// A device unplugged mid-claim can't be put back, and replugged it can report Melodia's level as
/// its own. The level from before the first claim is what the next release restores.
#[test]
fn a_restore_that_fails_is_kept_for_the_next_claim() -> Result<(), Refused> {
    let device = FakeDevice::at(0.6);
    let claim = HardwareVolume::take(device.clone(), "unplugged", 0.2)?;
    device.refuses.set(true);
    drop(claim);
    device.refuses.set(false);
    assert_eq!(device.level.get().to_bits(), 0.2_f32.to_bits(), "a refused restore changed it");

    drop(HardwareVolume::take(device.clone(), "unplugged", 0.2)?);

    assert_eq!(device.level.get().to_bits(), 0.6_f32.to_bits());
    Ok(())
}

/// Each set is a call into the system made on the writer's thread, so a slider drag reaches the
/// device at most once per interval. A move held back arrives late, never lost.
#[test]
fn a_moving_volume_reaches_the_device_at_most_once_per_interval() -> Result<(), Refused> {
    let device = FakeDevice::at(0.5);
    let mut claim = HardwareVolume::take(device.clone(), "follow-pacing", 0.3)?;
    let set_at = Instant::now() + FOLLOW_INTERVAL;
    claim.follow_at(0.2, set_at)?;

    claim.follow_at(0.1, set_at + FOLLOW_INTERVAL / 2)?;
    let held_back = device.level.get();
    claim.follow_at(0.1, set_at + FOLLOW_INTERVAL)?;

    assert_eq!(held_back.to_bits(), 0.2_f32.to_bits(), "a move inside the interval landed");
    assert_eq!(device.level.get().to_bits(), 0.1_f32.to_bits(), "the held move never landed");
    Ok(())
}

/// A system move handed back returns as the slider's rounding of it. Writing that would undo any
/// move the system made meanwhile, so a level the device already sits at is never written.
#[test]
fn a_followed_move_is_not_written_back_over_the_system() -> Result<(), Refused> {
    let device = FakeDevice::at(0.5);
    let mut claim = HardwareVolume::take(device.clone(), "write-back", 0.3)?;
    let watched_at = Instant::now() + WATCH_INTERVAL;
    device.moved_to(0.4249);
    let reported = claim.take_move_at(watched_at)?;
    let sets = device.sets.get();

    claim.follow_at(0.42, watched_at + FOLLOW_INTERVAL)?;

    assert_eq!(reported, Some(f64::from(0.4249_f32)));
    assert_eq!(device.sets.get(), sets, "the slider's rounding was written back");
    assert_eq!(device.level.get().to_bits(), 0.4249_f32.to_bits());
    Ok(())
}

/// A device rounds a level Melodia set to its own steps, and reading that back must not count as
/// the system moving it. A real move is reported once, and only once the watch is due.
#[test]
fn only_a_move_made_outside_melodia_is_reported_and_only_once() -> Result<(), Refused> {
    let device = FakeDevice::with_steps(0.5, 87.0);
    let mut claim = HardwareVolume::take(device.clone(), "move-watch", 0.3)?;
    let first = Instant::now() + WATCH_INTERVAL;

    let own_set = claim.take_move_at(first)?;
    device.moved_to(0.6);
    let too_soon = claim.take_move_at(first + Duration::from_millis(100))?;
    let moved = claim.take_move_at(first + WATCH_INTERVAL)?;
    let again = claim.take_move_at(first + WATCH_INTERVAL * 2)?;

    assert_eq!(own_set, None, "the read-back of Melodia's own set was reported");
    assert_eq!(too_soon, None, "the device was asked inside the watch interval");
    assert_eq!(moved, Some(f64::from(0.6_f32)));
    assert_eq!(again, None, "one move was reported twice");
    Ok(())
}
