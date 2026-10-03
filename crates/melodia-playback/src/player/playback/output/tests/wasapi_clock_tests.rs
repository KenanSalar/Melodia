//! Tests for time as the WASAPI writer counts it: a period in WASAPI's 100 ns units, the device's
//! clock read as time, and what two readings of that clock say about the device.

use std::time::Duration;

use super::{
    Clock, ClockReading, FUTILE_RESTARTS, STALL_TOLERANCE, StallWatch, clock_duration, from_hns,
    hns,
};

/// A clock counting in the 100 ns units the performance counter is read in, so both halves of a
/// reading count alike.
const CLOCK_HZ: u64 = 10_000_000;

/// 100 ns units in a millisecond.
const MS: u64 = 10_000;

/// The finest step the performance counter is read in.
const COUNTER_STEP: Duration = Duration::from_nanos(100);

/// The device's clock `played_ms` in, read when the performance counter stood at `at_ms`.
fn reading(played_ms: u64, at_ms: u64) -> ClockReading {
    ClockReading { played: played_ms * MS, at: at_ms * MS }
}

/// What `watch` answers to each of `readings`, in turn.
fn answers(watch: &mut StallWatch, readings: &[ClockReading]) -> Vec<Clock> {
    readings.iter().map(|&now| watch.read(now, CLOCK_HZ)).collect()
}

/// What a watch answers to each of `readings` where every `Stuck` restarts the stream, as the event
/// writer does, and how many restarts that took.
fn answers_restarting_when_stuck(readings: &[ClockReading]) -> (Vec<Clock>, u32) {
    let mut watch = StallWatch::default();
    let mut restarts = 0;
    let clocks = readings
        .iter()
        .map(|&now| {
            let clock = watch.read(now, CLOCK_HZ);
            if clock == Clock::Stuck {
                watch.restart();
                restarts += 1;
            }
            clock
        })
        .collect();
    (clocks, restarts)
}

/// What a stuck clock's third reading answers once `restarts` have gone by without it recovering.
fn third_stall_after(restarts: u32) -> Clock {
    let mut watch = StallWatch::default();
    for _ in 0..restarts {
        watch.restart();
    }
    watch.read(reading(10, 10), CLOCK_HZ);
    watch.read(reading(10, 30), CLOCK_HZ);
    watch.read(reading(10, 50), CLOCK_HZ)
}

/// What a second reading says where the device played 20 ms while the counter moved on 20 ms and
/// `behind` more.
fn behind_by(behind: Duration) -> Clock {
    let mut watch = StallWatch::default();
    watch.read(reading(10, 10), CLOCK_HZ);
    let behind_units = u64::try_from(behind.as_nanos() / 100).unwrap_or(u64::MAX);
    watch.read(ClockReading { played: 30 * MS, at: 30 * MS + behind_units }, CLOCK_HZ)
}

/// The tolerance's edge and a counter step either side. A clock exactly the tolerance behind is a
/// coarse one, and a step past it is a device that sat with nothing to play.
#[test]
fn a_clock_more_than_the_tolerance_behind_the_counter_is_a_stall() {
    let rows = [
        (Duration::ZERO, Clock::Moving),
        (STALL_TOLERANCE.saturating_sub(COUNTER_STEP), Clock::Moving),
        (STALL_TOLERANCE, Clock::Moving),
        (STALL_TOLERANCE + COUNTER_STEP, Clock::Stalled),
        (Duration::from_secs(1), Clock::Stalled),
    ];
    for (behind, expected) in rows {
        assert_eq!(behind_by(behind), expected, "{behind:?} behind");
    }
}

/// Two readings that disagree without the device having run dry. A stream's first period passes
/// before its clock moves at all, and a USB device's clock steps coarser than a sample, so it can
/// read ahead of the counter. A counter reading earlier than the last measures no time to have
/// stalled in.
#[test]
fn only_a_clock_standing_still_while_time_passes_is_a_stall() {
    let rows = [
        ("in step with the counter", reading(10, 10), reading(30, 30), Clock::Moving),
        ("still at zero a second in", reading(0, 10), reading(0, 1_010), Clock::Moving),
        ("ahead of the counter", reading(10, 10), reading(40, 30), Clock::Moving),
        ("the counter read earlier", reading(10, 30), reading(10, 10), Clock::Moving),
        ("standing still for 20 ms", reading(10, 10), reading(10, 30), Clock::Stalled),
    ];
    for (what, last, now, expected) in rows {
        let mut watch = StallWatch::default();
        watch.read(last, CLOCK_HZ);

        let clock = watch.read(now, CLOCK_HZ);

        assert_eq!(clock, expected, "{what}");
    }
}

/// One stall alone can be a driver whose clock steps coarser than the tolerance, so it is only
/// counted. A device a stall left running dry stalls at every reading after it, so the second in a
/// row is stuck, and an event-driven stream restarts for it.
#[test]
fn a_second_stall_in_a_row_reads_as_stuck() {
    let readings = [reading(10, 10), reading(10, 30), reading(10, 50), reading(10, 70)];

    let clocks = answers(&mut StallWatch::default(), &readings);

    assert_eq!(clocks, [Clock::Moving, Clock::Stalled, Clock::Stuck, Clock::Stuck]);
}

/// A device that keeps up between two stalls is recovering by itself, as the ALC897 does, and a
/// restart would cost it a period of silence for nothing.
#[test]
fn a_reading_that_keeps_up_starts_the_run_of_stalls_over() {
    let readings = [reading(10, 10), reading(10, 30), reading(30, 50), reading(30, 70)];

    let clocks = answers(&mut StallWatch::default(), &readings);

    assert_eq!(clocks, [Clock::Moving, Clock::Stalled, Clock::Moving, Clock::Stalled]);
}

/// A restart sets the device's clock back to zero. Measured against the last reading before it,
/// the first one after would read the reset as a second stall and restart the stream again.
#[test]
fn a_restart_measures_nothing_against_the_clock_before_it() {
    let mut watch = StallWatch::default();
    watch.read(reading(100, 100), CLOCK_HZ);
    watch.read(reading(100, 120), CLOCK_HZ);
    watch.restart();

    let clock = watch.read(reading(5, 140), CLOCK_HZ);

    assert_eq!(clock, Clock::Moving);
}

/// The cap's edge and a restart either side. A restart the clock never recovered from is one more
/// period of silence that bought nothing, so past the cap a stuck clock is only counted.
#[test]
fn a_stuck_clock_asks_for_no_restart_once_the_cap_is_spent() {
    let rows = [
        (FUTILE_RESTARTS - 1, Clock::Stuck),
        (FUTILE_RESTARTS, Clock::Stalled),
        (FUTILE_RESTARTS + 1, Clock::Stalled),
    ];
    for (restarts, expected) in rows {
        assert_eq!(third_stall_after(restarts), expected, "{restarts} restarts without recovery");
    }
}

/// A clock stepping coarser than a period stands still at every reading but the step, which reads
/// as stuck however often the stream restarts. Left uncapped, the writer would restart it every few
/// readings for the life of the stream. The clock's first step off zero after a reset is not
/// recovery, nothing being measured against a zero clock; only a reading that keeps up with the one
/// before it is.
#[test]
fn a_clock_no_restart_brings_back_is_restarted_only_up_to_the_cap() {
    let readings = [
        reading(10, 10),
        reading(10, 30),
        reading(10, 50),
        reading(0, 60),
        reading(10, 70),
        reading(10, 90),
        reading(10, 110),
        reading(0, 120),
        reading(10, 130),
        reading(10, 150),
        reading(10, 170),
        reading(10, 190),
    ];

    let answered = answers_restarting_when_stuck(&readings);

    let clocks = vec![
        Clock::Moving,
        Clock::Stalled,
        Clock::Stuck,
        Clock::Moving,
        Clock::Moving,
        Clock::Stalled,
        Clock::Stuck,
        Clock::Moving,
        Clock::Moving,
        Clock::Stalled,
        Clock::Stalled,
        Clock::Stalled,
    ];
    assert_eq!(answered, (clocks, FUTILE_RESTARTS));
}

/// A device a restart brings back, as the UMC22 in event mode is, keeps its restarts however many
/// stalls the stream meets, since each recovery starts the count over. Three here, one past the cap.
#[test]
fn a_clock_that_recovers_after_each_restart_is_restarted_every_time() {
    let readings = [
        reading(10, 10),
        reading(10, 30),
        reading(10, 50),
        reading(0, 60),
        reading(10, 70),
        reading(30, 90),
        reading(30, 110),
        reading(30, 130),
        reading(0, 140),
        reading(10, 150),
        reading(30, 170),
        reading(30, 190),
        reading(30, 210),
    ];

    let answered = answers_restarting_when_stuck(&readings);

    let clocks = vec![
        Clock::Moving,
        Clock::Stalled,
        Clock::Stuck,
        Clock::Moving,
        Clock::Moving,
        Clock::Moving,
        Clock::Stalled,
        Clock::Stuck,
        Clock::Moving,
        Clock::Moving,
        Clock::Moving,
        Clock::Stalled,
        Clock::Stuck,
    ];
    assert_eq!(answered, (clocks, FUTILE_RESTARTS + 1));
}

/// A device counts its clock at a rate it names beside it, so a reading is time only once divided
/// by that rate. A fraction of a nanosecond rounds down, and past what nanoseconds hold it
/// saturates rather than wrapping.
#[test]
fn a_device_clock_reads_as_time_at_the_rate_it_counts() {
    let rows = [
        (48_000, 48_000, Duration::from_secs(1)),
        (441, 44_100, Duration::from_millis(10)),
        (10_000_000, CLOCK_HZ, Duration::from_secs(1)),
        (1, 3, Duration::from_nanos(333_333_333)),
        (u64::MAX, 1, Duration::from_nanos(u64::MAX)),
    ];
    for (ticks, clock_hz, expected) in rows {
        assert_eq!(clock_duration(ticks, clock_hz), expected, "{ticks} ticks at {clock_hz} Hz");
    }
}

/// The period chips, in the 100 ns units WASAPI takes them in, and back unchanged: the writer
/// paces a polled claim off `from_hns` of the period the device agreed to.
#[test]
fn every_period_chip_goes_to_hundred_nanosecond_units_and_back_unchanged() {
    let rows = [(5, 50_000), (10, 100_000), (20, 200_000), (50, 500_000), (100, 1_000_000)];
    for (millis, expected_hns) in rows {
        let period = Duration::from_millis(millis);

        let there = hns(period);
        let back = from_hns(there);

        assert_eq!((there, back), (expected_hns, period), "{millis} ms");
    }
}

/// A duration finer than WASAPI's unit truncates, and one too long for it saturates rather than
/// wrapping into a negative period.
#[test]
fn a_duration_outside_what_wasapi_counts_truncates_or_saturates() {
    let rows = [(Duration::from_nanos(99), 0), (Duration::MAX, i64::MAX)];
    for (duration, expected_hns) in rows {
        assert_eq!(hns(duration), expected_hns, "{duration:?}");
    }
}

/// A negative period can only be a driver's nonsense, and reads as none rather than as a panic.
/// One too long to count in nanoseconds saturates.
#[test]
fn a_period_outside_what_nanoseconds_hold_reads_as_zero_or_saturates() {
    let longest = Duration::from_nanos(u64::MAX);
    let rows = [(-1, Duration::ZERO), (i64::MIN, Duration::ZERO), (i64::MAX, longest)];
    for (period_hns, expected) in rows {
        assert_eq!(from_hns(period_hns), expected, "{period_hns}");
    }
}
