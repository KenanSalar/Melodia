//! Where the song is between the position channel's reports, which land about once a second.
//!
//! **Within a run of play on one track the clock doesn't step back**, which would unlight words: a
//! report landing behind what was shown holds the clock there until the song catches up, and one
//! further back than [`DRIFT_SLACK_MS`] is a seek, taken as it stands.

use std::cell::Cell;
use std::time::Instant;

/// How far behind what was shown a report may land and still be drift: an emit carries the last
/// poll's position, a start or seek holds the device's clock for its lead, and the panel reads a
/// report a tick late.
const DRIFT_SLACK_MS: f64 = 1_000.0;

/// What the `Player` global says on one pass.
#[derive(Clone, Copy)]
pub(super) struct Report {
    pub(super) track_id: i32,
    pub(super) position_ms: i32,
    pub(super) playing: bool,
    pub(super) speed: f64,
}

pub(super) struct SongClock {
    /// The track at the last pass: a report for another one is an op's, however near it lands.
    track_id: Cell<i32>,
    /// The report last taken, so one read on many passes is taken once.
    seen_ms: Cell<i32>,
    anchor_ms: Cell<f64>,
    anchor_at: Cell<Instant>,
    /// Whether the transport played at the last pass: a report landing on a stopped clock is an
    /// op's, not drift.
    running: Cell<bool>,
    speed: Cell<f64>,
    floor_ms: Cell<f64>,
    /// When the screen last took the clock's answer, which a pause or a report is judged against:
    /// the transport may have stopped anywhere since, and a paused panel ticks only now and then.
    shown_at: Cell<Instant>,
}

impl SongClock {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            track_id: Cell::new(0),
            seen_ms: Cell::new(0),
            anchor_ms: Cell::new(0.0),
            anchor_at: Cell::new(now),
            running: Cell::new(false),
            speed: Cell::new(1.0),
            floor_ms: Cell::new(0.0),
            shown_at: Cell::new(now),
        }
    }

    pub(super) fn observe(&self, report: Report, now: Instant) {
        let shown = self.position(self.shown_at.get());
        let speed = report.speed.max(0.0);
        let speed_moved = speed.to_bits() != self.speed.get().to_bits();
        let track_moved = report.track_id != self.track_id.get();
        if track_moved || report.position_ms != self.seen_ms.get() {
            self.seen_ms.set(report.position_ms);
            self.take(report, shown, now);
        } else if report.playing != self.running.get() || speed_moved {
            // A pause freezes what was shown; a press or a new speed counts on from it.
            self.anchor(shown, now);
        }
        self.track_id.set(report.track_id);
        self.running.set(report.playing);
        self.speed.set(speed);
        self.shown_at.set(now);
    }

    /// Milliseconds into the track at `now`.
    pub(super) fn position(&self, now: Instant) -> f64 {
        let advanced = if self.running.get() {
            now.duration_since(self.anchor_at.get()).as_secs_f64() * 1000.0 * self.speed.get()
        } else {
            0.0
        };
        (self.anchor_ms.get() + advanced).max(self.floor_ms.get())
    }

    /// The report still on the global names the line the click left, so it isn't taken, and the
    /// seek's echo lands as drift.
    pub(super) fn jump_to(&self, at_ms: i32, now: Instant) {
        let at = f64::from(at_ms);
        self.floor_ms.set(at);
        self.anchor(at, now);
        self.shown_at.set(now);
    }

    fn take(&self, report: Report, shown: f64, now: Instant) {
        let reported = f64::from(report.position_ms);
        let same_run = self.running.get() && report.track_id == self.track_id.get();
        let drifted = same_run && (0.0..=DRIFT_SLACK_MS).contains(&(shown - reported));
        self.floor_ms.set(if drifted { shown } else { reported });
        self.anchor(reported, now);
    }

    fn anchor(&self, position_ms: f64, now: Instant) {
        self.anchor_ms.set(position_ms);
        self.anchor_at.set(now);
    }
}

#[cfg(test)]
#[path = "tests/clock_tests.rs"]
mod tests;
