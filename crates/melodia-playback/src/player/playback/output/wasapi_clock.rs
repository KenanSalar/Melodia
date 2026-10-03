//! Time as the WASAPI writer counts it: the 100 ns units periods are declared in, and the device's
//! own clock, read against the performance counter, which is where a stall shows.

use std::time::Duration;

/// How far the device's clock may fall behind the performance counter between two readings before
/// it counts as an underrun. A starved device holds its clock still while time passes; this covers
/// the steps its clock advances in, which a USB device takes coarser than a sample.
const STALL_TOLERANCE: Duration = Duration::from_millis(2);

/// Stalls in a row before the device counts as stuck. One alone can be a driver whose clock steps
/// coarser than [`STALL_TOLERANCE`], and restarting for it would cost a period of silence each
/// time; a device left running dry by a stall shows it at every reading after.
const STALLS_WHEN_STUCK: u32 = 2;

/// Restarts in a row the clock never recovered between, after which a stuck clock is only counted.
/// A restart that doesn't bring the clock back is a clock stepping coarser than a period or a stall
/// no restart cures, and either way another one only adds a period of silence.
const FUTILE_RESTARTS: u32 = 2;

/// One answer from the device's clock: what it had played, in its own ticks, and the performance
/// counter at the moment it read that, in 100 ns units.
#[derive(Clone, Copy)]
pub(super) struct ClockReading {
    pub(super) played: u64,
    pub(super) at: u64,
}

/// What a reading of the device's clock says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Clock {
    Moving,
    Stalled,
    /// Stalled at [`STALLS_WHEN_STUCK`] readings in a row, so it isn't recovering by itself, and
    /// fewer than [`FUTILE_RESTARTS`] restarts have gone by without it recovering.
    Stuck,
}

/// The device's clock across one stream's readings.
#[derive(Default)]
pub(super) struct StallWatch {
    last: Option<ClockReading>,
    stalls_in_a_row: u32,
    restarts_without_recovery: u32,
}

impl StallWatch {
    /// Take `now` off a clock counting `clock_hz` a second, answering what it says.
    pub(super) fn read(&mut self, now: ClockReading, clock_hz: u64) -> Clock {
        let compared = self.last.is_some_and(|last| last.played != 0);
        let stalled = self.last.is_some_and(|last| stood_still(last, now, clock_hz));
        self.last = Some(now);
        if compared && !stalled {
            self.restarts_without_recovery = 0;
        }
        self.stalls_in_a_row = if stalled { self.stalls_in_a_row + 1 } else { 0 };
        match self.stalls_in_a_row {
            0 => Clock::Moving,
            n if n < STALLS_WHEN_STUCK => Clock::Stalled,
            _ if self.restarts_without_recovery >= FUTILE_RESTARTS => Clock::Stalled,
            _ => Clock::Stuck,
        }
    }

    /// Start over against a clock a reset set back to zero, remembering that this restart has yet
    /// to bring it back.
    pub(super) fn restart(&mut self) {
        self.last = None;
        self.stalls_in_a_row = 0;
        self.restarts_without_recovery += 1;
    }
}

/// Whether the device sat with nothing to play between `last` and `now`: its clock advanced less
/// than the time that passed, by more than [`STALL_TOLERANCE`].
///
/// Read off the clock's own progress rather than against what was written: a device can halt its
/// clock with samples still queued, and come back from a stall with fewer queued than before, so a
/// count against what was written misses one stall and repeats another. Not asked before the clock
/// moves at all, since a stream's first period passes before it does.
fn stood_still(last: ClockReading, now: ClockReading, clock_hz: u64) -> bool {
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

#[cfg(test)]
#[path = "tests/wasapi_clock_tests.rs"]
mod tests;
