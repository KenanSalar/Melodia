//! Time as the WASAPI writer counts it: the 100 ns units periods are declared in, and the device's
//! own clock, read against the performance counter, which is where a stall shows.

use std::time::Duration;

/// How far the device's clock may fall behind the performance counter between two readings before
/// it counts as an underrun. A starved device holds its clock still while time passes; this covers
/// the steps its clock advances in, which a USB device takes coarser than a sample.
const STALL_TOLERANCE: Duration = Duration::from_millis(2);

/// One answer from the device's clock: what it had played, in its own ticks, and the performance
/// counter at the moment it read that, in 100 ns units.
#[derive(Clone, Copy)]
pub(super) struct ClockReading {
    pub(super) played: u64,
    pub(super) at: u64,
}

/// Whether the device sat with nothing to play between `last` and `now`: its clock advanced less
/// than the time that passed, by more than [`STALL_TOLERANCE`].
///
/// Read off the clock's own progress rather than against what was written: a device can halt its
/// clock with samples still queued, and come back from a stall with fewer queued than before, so a
/// count against what was written misses one stall and repeats another. Not asked before the clock
/// moves at all, since a stream's first period passes before it does.
pub(super) fn stood_still(last: ClockReading, now: ClockReading, clock_hz: u64) -> bool {
    if last.played == 0 {
        return false;
    }
    let elapsed = from_hns(i64::try_from(now.at.saturating_sub(last.at)).unwrap_or(i64::MAX));
    let played = clock_duration(now.played.saturating_sub(last.played), clock_hz);
    elapsed.saturating_sub(played) > STALL_TOLERANCE
}

/// `ticks` of a clock counting `clock_hz` a second, as a duration.
pub(super) fn clock_duration(ticks: u64, clock_hz: u64) -> Duration {
    let nanos = u128::from(ticks) * 1_000_000_000 / u128::from(clock_hz);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// `duration` in the 100 ns units WASAPI counts periods in.
pub(super) fn hns(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX)
}

/// [`hns`]'s way back.
pub(super) fn from_hns(hns: i64) -> Duration {
    Duration::from_nanos(u64::try_from(hns).unwrap_or(0).saturating_mul(100))
}
