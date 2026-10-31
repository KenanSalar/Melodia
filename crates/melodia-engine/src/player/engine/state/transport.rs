//! The track transport: every builder a play, pause, skip, seek or end-of-stream goes through, and
//! the one writer of what "now playing this track" means.

use std::sync::Arc;

use super::volume_to_amplitude;
use super::{
    AwaitedStart, MAX_SPEED, MAX_VOLUME, MIN_SPEED, PlayerAction, PlayerState, RESTART_THRESHOLD_MS,
};
use crate::player::engine::types::{PlaybackSource, PlaybackStatus};
use melodia_core::entities::track::TrackSummary;
use melodia_playback::player::playback::crossfade::CrossfadeDecision;
use melodia_playback::player::playback::replaygain::TrackReplayGain;

impl PlayerState {
    /// The volume as a backend amplitude, through the [`volume_to_amplitude`] MPRIS shares.
    pub fn effective_volume(&self) -> f64 {
        volume_to_amplitude(self.volume, self.is_muted)
    }

    /// Declines for a station: pausing dropped its connection, and reopening one is a network
    /// round trip that can't happen under the state lock, so `library::playback::player_play`
    /// takes that branch first.
    pub fn build_play_actions(&mut self) -> Vec<PlayerAction> {
        if self.is_radio() {
            return vec![];
        }
        if self.status == PlaybackStatus::Paused {
            self.status = PlaybackStatus::Playing;
            vec![PlayerAction::Resume]
        } else {
            resume_from_stopped(self)
        }
    }

    /// Starts a paused track its deck no longer holds (a long pause gave the device back) again
    /// from its position. The deck is the backend's to answer, so `library::playback` picks this
    /// over [`Self::build_play_actions`].
    pub fn build_replay_actions(&mut self) -> Vec<PlayerAction> {
        if self.status != PlaybackStatus::Paused {
            return vec![];
        }
        replay_current_track(self).unwrap_or_default()
    }

    /// `fade_ms` is the pause-fade length, `0` with the setting off. A station's connection is
    /// dropped, since a held-open socket back-pressures the server and resumes on stale audio.
    pub fn build_pause_actions(&mut self, fade_ms: u64) -> Vec<PlayerAction> {
        if self.is_radio() && self.status != PlaybackStatus::Stopped {
            self.status = PlaybackStatus::Paused;
            self.end_stream_session();
            return vec![PlayerAction::Stop { fade_ms }];
        }
        if self.status == PlaybackStatus::Playing {
            self.status = PlaybackStatus::Paused;
            vec![PlayerAction::Pause { fade_ms }]
        } else {
            vec![]
        }
    }

    /// Takes a long-paused track off the deck so its exclusive device goes back to the system,
    /// leaving it paused for [`Self::build_replay_actions`]. The monitor decided outside the emit
    /// lock, so what it saw is checked again, a seek in the gap moving only the position.
    pub fn build_release_actions(&mut self, decision: ReleaseDecision) -> Vec<PlayerAction> {
        let same_track = self.current_track().map(|track| track.id) == Some(decision.track_id);
        if self.status != PlaybackStatus::Paused
            || !same_track
            || self.position_ms != decision.position_ms
        {
            return vec![];
        }
        self.position_ms = decision.resume_ms;
        vec![PlayerAction::Stop { fade_ms: 0 }]
    }

    /// Back to the top of a track that stays seated, a pause being what keeps the position. A
    /// station is forgotten outright, handing back the queue left untouched underneath it.
    pub fn build_stop_actions(&mut self, fade_ms: u64) -> Vec<PlayerAction> {
        self.status = PlaybackStatus::Stopped;
        self.position_ms = 0;
        if self.is_radio() {
            self.end_stream_session();
            self.source = None;
        }
        vec![PlayerAction::Stop { fade_ms }]
    }

    /// The seek for whatever track is seated, which rides along because the backend rebuilds the
    /// source to move it.
    fn seek_action(&self, position_ms: u64) -> Option<PlayerAction> {
        let track = self.source.as_ref().and_then(PlaybackSource::track)?;
        Some(PlayerAction::Seek {
            position_ms,
            file_path: track.file_path.clone(),
            replaygain: track.as_ref().into(),
        })
    }

    /// Moves the position, and the deck with it where a track is seated: `source_allows` passes
    /// on an absent source, so this is reachable before anything has played.
    fn build_move_to_actions(&mut self, position_ms: u64) -> Vec<PlayerAction> {
        let seek = self.seek_action(position_ms);
        self.position_ms = position_ms;
        seek.into_iter().collect()
    }

    pub fn build_seek_actions(&mut self, position_ms: u64) -> Vec<PlayerAction> {
        // A live stream has no timeline to land on; the position is elapsed listening time.
        if !self.source_allows(PlaybackSource::is_seekable) {
            return vec![];
        }
        self.build_move_to_actions(position_ms)
    }

    pub fn build_next_actions(&mut self) -> Vec<PlayerAction> {
        if !self.source_allows(PlaybackSource::advances_queue) {
            return vec![];
        }
        let mut actions = vec![];
        let was_paused = self.status == PlaybackStatus::Paused;

        if let Some(track) = self.current_track()
            && self.duration_ms > 0
            && self.position_ms < self.duration_ms / 2
        {
            actions.push(PlayerAction::UpdateSkipCount(track.id));
        }

        match self.queue.advance_skip() {
            Some(id) => actions.extend(self.start_queued(id, was_paused, 0)),
            None => actions.extend(stop_end_of_queue(self)),
        }

        actions
    }

    /// Starts `id`, the queue's new current track, from its window record, or once the window task
    /// has fetched it. `skipped` counts the unplayable tracks already passed, carried through a
    /// deferral for `execute_actions`' lap guard.
    pub fn start_queued(&mut self, id: i64, re_pause: bool, skipped: usize) -> Vec<PlayerAction> {
        let Some(track) = self.record(id) else {
            return self.await_record(id, re_pause, skipped);
        };
        let mut actions = play_track_inner(self, track, None);
        self.restore_paused(re_pause, &mut actions);
        actions
    }

    /// Owe the start of `id` to the window task. `Loading` on an empty deck is what keeps the
    /// monitor from ticking, or from ending a drained stream a second time, while it is fetched.
    fn await_record(&mut self, id: i64, re_pause: bool, skipped: usize) -> Vec<PlayerAction> {
        if self.queue.current_index.is_none() {
            return stop_end_of_queue(self);
        }
        end_evicted_session(self);
        self.status = PlaybackStatus::Loading;
        self.source = None;
        self.duration_ms = 0;
        self.position_ms = 0;
        vec![
            PlayerAction::Stop { fade_ms: 0 },
            PlayerAction::StartWhenResolved(AwaitedStart { id, re_pause, skipped }),
        ]
    }

    /// Whether a deferred start can run now: nothing has started or stopped since, and the queue's
    /// current track has its record in the window, or the queue has emptied under it.
    pub fn still_awaits(&self) -> bool {
        self.status == PlaybackStatus::Loading
            && self.source.is_none()
            && self.queue.current_id().is_none_or(|id| self.window.get(id).is_some())
    }

    /// Runs the owed start on whatever the queue's current slot holds by now. Never defers again,
    /// so a record that never lands can't loop the window task.
    pub fn resume_awaited(&mut self, awaited: AwaitedStart) -> Vec<PlayerAction> {
        if !self.still_awaits() {
            return vec![];
        }
        match self.queue.current_id() {
            Some(id) => self.start_queued(id, awaited.re_pause, awaited.skipped),
            None => stop_end_of_queue(self),
        }
    }

    /// Never faded: the `PlayMedia` before it just started the deck, and a fade would ramp the
    /// incoming track down audibly instead of pausing it.
    pub(super) fn restore_paused(&mut self, was_paused: bool, actions: &mut Vec<PlayerAction>) {
        if was_paused {
            self.status = PlaybackStatus::Paused;
            actions.push(PlayerAction::Pause { fade_ms: 0 });
        }
    }

    pub fn build_previous_actions(&mut self) -> Vec<PlayerAction> {
        if !self.source_allows(PlaybackSource::advances_queue) {
            return vec![];
        }
        let was_paused = self.status == PlaybackStatus::Paused;

        if self.position_ms > RESTART_THRESHOLD_MS {
            return self.build_move_to_actions(0);
        }

        match self.queue.previous() {
            Some(id) => self.start_queued(id, was_paused, 0),
            None => self.build_move_to_actions(0),
        }
    }

    pub fn build_set_volume_actions(&mut self, level: u32) -> Vec<PlayerAction> {
        self.volume = level.min(MAX_VOLUME);
        self.is_muted = false;
        vec![PlayerAction::SetVolume(self.effective_volume())]
    }

    /// Returns whether setting `level` would change nothing. Never while muted, setting the held
    /// level being how the user unmutes.
    pub fn holds_volume(&self, level: u32) -> bool {
        self.volume == level && !self.is_muted
    }

    /// Returns the level a step of `delta` lands on, clamped to `0..=MAX_VOLUME`. Steps from the
    /// stored level while muted, as [`Self::build_set_volume_actions`] then unmutes.
    pub fn stepped_volume(&self, delta: i32) -> u32 {
        self.volume.saturating_add_signed(delta).min(MAX_VOLUME)
    }

    pub fn build_step_volume_actions(&mut self, delta: i32) -> Vec<PlayerAction> {
        self.build_set_volume_actions(self.stepped_volume(delta))
    }

    pub fn build_toggle_mute_actions(&mut self) -> Vec<PlayerAction> {
        let new_muted = !self.is_muted;
        if new_muted {
            self.pre_mute_volume = self.volume;
        }
        self.is_muted = new_muted;
        vec![PlayerAction::SetVolume(self.effective_volume())]
    }

    /// Counts the play and advances, or stops where the queue ends or the sleep timer's
    /// end-of-track mode is armed, disarming it. It stops in place because `player_pause` would
    /// take the state lock this already runs under.
    pub fn build_end_of_stream_actions(&mut self) -> Vec<PlayerAction> {
        // A station drains only once its reconnect budget is spent, and wandering into the
        // library's queue then would be a silent change of source.
        if !self.source_allows(PlaybackSource::advances_queue) {
            return self.build_stop_actions(0);
        }

        let mut actions = Vec::with_capacity(4);

        if let Some(track) = self.current_track() {
            actions.push(PlayerAction::UpdatePlayCount(track.id));
        }

        if self.pause_after_current_track {
            self.pause_after_current_track = false;
            actions.extend(stop_end_of_queue(self));
            return actions;
        }

        match self.queue.advance() {
            Some(id) => actions.extend(self.start_queued(id, false, 0)),
            None => actions.extend(stop_end_of_queue(self)),
        }

        actions
    }

    /// Starts the next track on the other deck as this one fades out over `fade_ms`, the state
    /// advancing at fade start. Empty where there is no next record or the decision went stale.
    ///
    /// The monitor decides under the state lock but lands here after `exec_lock`, so any control
    /// op can come between. The status guards a pause, the track id a pick or skip, and the
    /// position a seek or restart, which keep the other two: the monitor writes the position just
    /// before deciding, so only an op that must abort can move it.
    pub fn build_crossfade_actions(&mut self, decision: CrossfadeDecision) -> Vec<PlayerAction> {
        let mut actions = Vec::with_capacity(2);

        if !self.source_allows(PlaybackSource::advances_queue) {
            return actions;
        }

        let Some(outgoing_id) = self.current_track().map(|t| t.id) else {
            return actions;
        };
        if self.status != PlaybackStatus::Playing
            || Some(outgoing_id) != decision.track_id
            || self.position_ms != decision.position_ms
        {
            return actions;
        }

        // No record means no crossfade: a fade can't wait on a fetch, and the end-of-stream the
        // track drains to can.
        let Some(track) = self.queue.peek_next().and_then(|id| self.record(id)) else {
            return actions;
        };
        self.queue.advance();

        // Counted at fade start, once there is somewhere to go.
        actions.push(PlayerAction::UpdatePlayCount(outgoing_id));

        let start = begin_track(self, track, None);

        actions.push(PlayerAction::BeginCrossfade {
            file_path: start.file_path,
            replaygain: start.replaygain,
            fade_ms: decision.fade_ms,
            volume: start.volume,
            speed: start.speed,
        });
        actions
    }

    /// Moves the state onto the `staged` track the deck has handed over to. Advances
    /// unconditionally, so the caller claims the handover first
    /// (`PlaybackEngine::claim_gapless_handover`). Where an edit in the last `PRELOAD_LEAD_MS`
    /// moved the queue's next, the queue wins, cut to at most a poll into the staged track.
    pub fn build_gapless_advance_actions(
        &mut self,
        staged: Option<Arc<TrackSummary>>,
    ) -> Vec<PlayerAction> {
        let mut actions = Vec::with_capacity(2);

        if let Some(track) = self.current_track() {
            actions.push(PlayerAction::UpdatePlayCount(track.id));
        }

        match (self.queue.advance(), staged) {
            (Some(id), Some(track)) if track.id == id => {
                self.position_ms = 0;
                self.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
                self.source = Some(PlaybackSource::Track(track));
            }
            (Some(id), _) => actions.extend(self.start_queued(id, false, 0)),
            (None, _) => actions.extend(stop_end_of_queue(self)),
        }

        actions
    }

    pub fn build_set_speed_actions(&mut self, speed: f64) -> Vec<PlayerAction> {
        // A live mount runs at 1.0 or its ring starves or overruns. Refused rather than clamped,
        // so the transport shows the speed the deck runs at.
        if !self.source_allows(PlaybackSource::has_variable_speed) {
            return vec![];
        }
        let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        self.playback_speed = speed;
        vec![PlayerAction::SetSpeed(speed)]
    }
}

/// What the monitor saw when it decided a pause had held the device long enough, for
/// [`PlayerState::build_release_actions`] to check again under the emit lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseDecision {
    pub track_id: i64,
    /// The state's own position, which only a seek moves while paused.
    pub position_ms: u64,
    /// Where the deck stopped pulling, read before the clear. The state's position is the last
    /// tick's and trails it, so play restarts from here.
    pub resume_ms: u64,
}

/// What a start action needs about the track [`begin_track`] pointed the state at.
struct TrackStart {
    file_path: String,
    replaygain: TrackReplayGain,
    volume: f64,
    speed: f64,
    /// Clamped; `None` is from the top.
    start_position_ms: Option<u64>,
}

/// Points `state` at `track`: the one writer of what "now playing this" means, shared by
/// [`play_track_inner`] and the crossfade so the two can't drift.
fn begin_track(
    state: &mut PlayerState,
    track: Arc<TrackSummary>,
    start_position_ms: Option<u64>,
) -> TrackStart {
    end_evicted_session(state);

    state.status = PlaybackStatus::Playing;
    state.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
    // Clamp to 500ms before end to avoid immediate EOS detection by the playback monitor.
    let max_resume_pos = state.duration_ms.saturating_sub(500);
    let clamped_pos = start_position_ms.map(|p| p.min(max_resume_pos)).filter(|&p| p > 0);
    state.position_ms = clamped_pos.unwrap_or(0);

    let start = TrackStart {
        file_path: track.file_path.clone(),
        replaygain: track.as_ref().into(),
        volume: state.effective_volume(),
        speed: state.playback_speed,
        start_position_ms: clamped_pos,
    };
    state.source = Some(PlaybackSource::Track(track));
    start
}

/// Ends a station's session as a track replaces it, or a connect in flight would pass its
/// generation check and start the station over the track.
fn end_evicted_session(state: &mut PlayerState) {
    if state.is_radio() {
        state.end_stream_session();
    }
}

/// Starts `track`, from `start_position_ms` where given.
pub fn play_track_inner(
    state: &mut PlayerState,
    track: Arc<TrackSummary>,
    start_position_ms: Option<u64>,
) -> Vec<PlayerAction> {
    let start = begin_track(state, track, start_position_ms);

    // No gapless preload here: the monitor stages one late, so a queue or repeat change mid-track
    // still counts.
    vec![PlayerAction::PlayMedia {
        file_path: start.file_path,
        volume: start.volume,
        speed: start.speed,
        start_position_ms: start.start_position_ms,
        replaygain: start.replaygain,
    }]
}

/// Stop at the end of the queue. Never faded: the track has already run out of audio, so there is
/// nothing left to fade, and a deferred clear would only delay the silence.
pub fn stop_end_of_queue(state: &mut PlayerState) -> Vec<PlayerAction> {
    state.build_stop_actions(0)
}

/// Replays the current track from its position, or starts the queue's current slot where none is
/// seated: a stopped station is forgotten, and the queue under it is what play hands back.
pub fn resume_from_stopped(state: &mut PlayerState) -> Vec<PlayerAction> {
    if state.status != PlaybackStatus::Stopped {
        return vec![];
    }
    if let Some(actions) = replay_current_track(state) {
        return actions;
    }
    match state.queue.current_id() {
        Some(id) => state.start_queued(id, false, 0),
        None => vec![],
    }
}

/// Start the track the state points at again from its position, or `None` where it points at
/// none. A stop and a paused track taken off the deck both come back this way.
fn replay_current_track(state: &mut PlayerState) -> Option<Vec<PlayerAction>> {
    let track = state.current_track().cloned()?;
    let resume_pos = (state.position_ms > 0).then_some(state.position_ms);
    Some(play_track_inner(state, track, resume_pos))
}
