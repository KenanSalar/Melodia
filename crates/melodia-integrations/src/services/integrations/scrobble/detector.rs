//! The track scrobble state machine, pure: view models and position ticks in, effects out, with
//! `now_ts` (UNIX seconds) an input. `tasks::scrobble` drives it and does the I/O.
//!
//! A play scrobbles when it *ends*, at a different track, a restart, a stop or shutdown, having
//! met [`scrobble_threshold_ms`]. Now-playing goes out once at each play's start.

use crate::services::integrations::scrobble::model::scrobble_threshold_ms;
use melodia_engine::player::engine::state::{PlayerViewModelLight, PositionTick};

/// A forward jump past this is a seek and adds no played time; the 500 ms poll leaves generous
/// headroom for a coalesced tick.
const SEEK_GUARD_MS: u64 = 5_000;

/// A position at or below this counts as back at the start.
const RESTART_EPSILON_MS: u64 = 1_500;

/// Progress needed before a drop to the start reads as a restart rather than start-up jitter.
const RESTART_MIN_MS: u64 = 3_000;

/// Ids rather than rows: the task fetches the row it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A play began.
    NowPlaying { track_id: i64 },
    /// A qualified play ended into a successor or a restart, stamped with its start.
    Scrobble { track_id: i64, timestamp: i64 },
    /// A qualified play ended at a stop or shutdown, stamped with its start.
    Finalize { track_id: i64, timestamp: i64 },
}

#[derive(Debug, Default)]
pub struct DetectorState {
    current_id: Option<i64>,
    current_duration_ms: u64,
    started_at: i64,
    accumulated_ms: u64,
    last_position_ms: u64,
}

impl DetectorState {
    pub fn new() -> Self {
        Self::default()
    }

    /// A same-id republish (volume, pause, loading) produces nothing, which is what holds
    /// now-playing to once per track start.
    pub fn on_view_model(&mut self, vm: Option<&PlayerViewModelLight>, now_ts: i64) -> Vec<Effect> {
        let Some(vm) = vm else {
            return Vec::new();
        };

        if vm.status == "stopped" || vm.current_track.is_none() {
            return self.end_current(true);
        }

        let Some(track) = vm.current_track.as_ref() else {
            return Vec::new();
        };
        let incoming_id = track.id;
        let incoming_duration = u64::try_from(track.duration_ms).unwrap_or(0);

        if self.current_id == Some(incoming_id) {
            // Only a duration unknown at the start to refine; ticks drive accumulation.
            if self.current_duration_ms == 0 {
                self.current_duration_ms = incoming_duration;
            }
            return Vec::new();
        }

        let mut effects = self.end_current(false);
        self.begin_play(incoming_id, incoming_duration, now_ts, 0);
        effects.push(Effect::NowPlaying { track_id: incoming_id });
        effects
    }

    /// Accumulates real playback only, a seek or a frozen paused position adding nothing. A drop
    /// back to the start is a restart: the prior play ends and a fresh one begins.
    pub fn on_position(&mut self, tick: &PositionTick, now_ts: i64) -> Vec<Effect> {
        let Some(id) = self.current_id else {
            return Vec::new();
        };
        if self.current_duration_ms == 0 && tick.duration_ms > 0 {
            self.current_duration_ms = tick.duration_ms;
        }

        if tick.position_ms <= RESTART_EPSILON_MS && self.last_position_ms >= RESTART_MIN_MS {
            let mut effects = Vec::new();
            if self.qualified() {
                effects.push(Effect::Scrobble { track_id: id, timestamp: self.started_at });
            }
            let duration = self.current_duration_ms;
            self.begin_play(id, duration, now_ts, tick.position_ms);
            effects.push(Effect::NowPlaying { track_id: id });
            return effects;
        }

        let delta = tick.position_ms.saturating_sub(self.last_position_ms);
        if (1..=SEEK_GUARD_MS).contains(&delta) {
            self.accumulated_ms += delta;
        }
        self.last_position_ms = tick.position_ms;
        Vec::new()
    }

    pub fn on_shutdown(&mut self) -> Vec<Effect> {
        self.end_current(true)
    }

    fn end_current(&mut self, terminal: bool) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(id) = self.current_id
            && self.qualified()
        {
            let timestamp = self.started_at;
            effects.push(if terminal {
                Effect::Finalize { track_id: id, timestamp }
            } else {
                Effect::Scrobble { track_id: id, timestamp }
            });
        }
        self.current_id = None;
        self.current_duration_ms = 0;
        self.accumulated_ms = 0;
        self.last_position_ms = 0;
        effects
    }

    fn begin_play(&mut self, id: i64, duration_ms: u64, now_ts: i64, position_ms: u64) {
        self.current_id = Some(id);
        self.current_duration_ms = duration_ms;
        self.started_at = now_ts;
        self.accumulated_ms = 0;
        self.last_position_ms = position_ms;
    }

    fn qualified(&self) -> bool {
        matches!(scrobble_threshold_ms(self.current_duration_ms), Some(t) if self.accumulated_ms >= t)
    }
}

#[cfg(test)]
#[path = "tests/detector_tests.rs"]
mod tests;
