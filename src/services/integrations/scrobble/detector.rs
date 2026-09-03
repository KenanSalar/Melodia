//! Pure scrobble-detection state machine. Turns the player's view-model +
//! position ticks into scrobble / now-playing decisions, mirroring the
//! pure-function style of `player::engine::handlers::evaluate_playing_tick`: a `&mut`
//! state plus value inputs in, a decision value out — no I/O, locks, async, or
//! clock reads (the `now_ts` UNIX-seconds timestamp is an input). The impure
//! task in `tasks::scrobble` drives it and performs the DB fetch + service
//! calls.
//!
//! The scrobble rule (both services agree): a track over 30 s scrobbles once it
//! has played at least half its length or 4 minutes, whichever comes first
//! (`model::scrobble_threshold_ms`). A scrobble is emitted when a play *ends* —
//! a different track starts, the same track restarts, playback stops, or the app
//! shuts down — and its accumulated played-time met the threshold (the classic
//! "scrobble on completion / skip-past-halfway" model). "Now playing" is sent
//! once at each play's start.

use crate::player::engine::state::{PlayerViewModelLight, PositionTick};
use crate::services::integrations::scrobble::model::scrobble_threshold_ms;

/// Largest forward position jump (ms) counted as real playback. A tick advancing
/// more than this is a seek and adds nothing to played time; the ~500 ms poll
/// interval leaves generous headroom for a coalesced tick.
const SEEK_GUARD_MS: u64 = 5_000;

/// A position at or below this (ms) counts as "back at the start" for restart
/// detection.
const RESTART_EPSILON_MS: u64 = 1_500;

/// Minimum prior progress (ms) before a drop into `RESTART_EPSILON_MS` reads as a
/// genuine restart rather than start-up jitter.
const RESTART_MIN_MS: u64 = 3_000;

/// One thing the detector decided the task must act on. Track ids are carried so
/// the impure task can fetch/enrich the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A play began — send an ephemeral "now playing" for this track.
    NowPlaying { track_id: i64 },
    /// A play ended via a successor track or a restart, and it qualified —
    /// enqueue a scrobble stamped with when it started.
    Scrobble { track_id: i64, timestamp: i64 },
    /// A play ended terminally (stop / shutdown) and it qualified — enqueue a
    /// scrobble stamped with when it started.
    Finalize { track_id: i64, timestamp: i64 },
}

/// Running state for the play currently being tracked.
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

    /// React to a published view-model: a track change scrobbles the outgoing
    /// play (if qualified) and now-plays the incoming; a stop finalizes. A
    /// same-id republish (volume/pause/loading) produces nothing — this is the
    /// "exactly one now-playing per track start" gate.
    pub fn on_view_model(&mut self, vm: Option<&PlayerViewModelLight>, now_ts: i64) -> Vec<Effect> {
        let Some(vm) = vm else {
            return Vec::new();
        };

        // A stop (or a vanished current track) ends the current play terminally.
        if vm.status == "stopped" || vm.current_track.is_none() {
            return self.end_current(true);
        }

        let Some(track) = vm.current_track.as_ref() else {
            return Vec::new();
        };
        let incoming_id = track.id;
        let incoming_duration = u64::try_from(track.duration_ms).unwrap_or(0);

        if self.current_id == Some(incoming_id) {
            // Same track republished: refine an unknown-at-start duration, else
            // nothing (position ticks drive accumulation).
            if self.current_duration_ms == 0 {
                self.current_duration_ms = incoming_duration;
            }
            return Vec::new();
        }

        // A different (or first) track: end the outgoing play via transition,
        // then begin the incoming.
        let mut effects = self.end_current(false);
        self.begin_play(incoming_id, incoming_duration, now_ts, 0);
        effects.push(Effect::NowPlaying {
            track_id: incoming_id,
        });
        effects
    }

    /// React to a ~1 Hz position tick, correlated against the current track id.
    /// Accumulates only real playback (a seek or a paused, frozen position adds
    /// nothing); a drop back to the start is a restart — end the prior play and
    /// begin a fresh one on the same track.
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
                effects.push(Effect::Scrobble {
                    track_id: id,
                    timestamp: self.started_at,
                });
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

    /// Finalize the current play on shutdown.
    pub fn on_shutdown(&mut self) -> Vec<Effect> {
        self.end_current(true)
    }

    /// End the current play, emitting a `Scrobble` (transition) or `Finalize`
    /// (terminal) when it qualified, then clearing state.
    fn end_current(&mut self, terminal: bool) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(id) = self.current_id
            && self.qualified()
        {
            let timestamp = self.started_at;
            effects.push(if terminal {
                Effect::Finalize {
                    track_id: id,
                    timestamp,
                }
            } else {
                Effect::Scrobble {
                    track_id: id,
                    timestamp,
                }
            });
        }
        self.current_id = None;
        self.current_duration_ms = 0;
        self.accumulated_ms = 0;
        self.last_position_ms = 0;
        effects
    }

    /// Start tracking a new play.
    fn begin_play(&mut self, id: i64, duration_ms: u64, now_ts: i64, position_ms: u64) {
        self.current_id = Some(id);
        self.current_duration_ms = duration_ms;
        self.started_at = now_ts;
        self.accumulated_ms = 0;
        self.last_position_ms = position_ms;
    }

    /// Whether the current play has met the scrobble threshold.
    fn qualified(&self) -> bool {
        matches!(scrobble_threshold_ms(self.current_duration_ms), Some(t) if self.accumulated_ms >= t)
    }
}

#[cfg(test)]
#[path = "tests/detector_tests.rs"]
mod tests;
