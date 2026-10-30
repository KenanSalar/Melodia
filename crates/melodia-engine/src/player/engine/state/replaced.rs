//! A play's replace of a queue built by hand, held so its undo can put back the queue and what was
//! playing over it.

use super::{PlayerAction, PlayerState, play_track_inner, restore_station};
use crate::player::engine::queue::ClearedQueue;
use crate::player::engine::types::{PlaybackSource, PlaybackStatus};

/// What a play took: the queue, moved, and the playback it ended.
#[derive(Debug)]
pub struct ReplacedQueue {
    queue: ClearedQueue,
    playback: HeldPlayback,
}

/// The source itself rather than a queue row, a playing track whose row has left being in none.
#[derive(Debug)]
struct HeldPlayback {
    source: Option<PlaybackSource>,
    status: PlaybackStatus,
    position_ms: u64,
    duration_ms: u64,
}

impl PlayerState {
    /// Makes `ids` the queue with the row at `start` current, handing back what undoes it where
    /// [`QueueState::replace`](crate::player::engine::queue::QueueState::replace) held the old one.
    pub fn replace_queue(&mut self, ids: &[i64], start: usize) -> Option<ReplacedQueue> {
        let playback = HeldPlayback {
            source: self.source.clone(),
            status: self.status,
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
        };
        let queue = self.queue.replace(ids, start)?;
        Some(ReplacedQueue { queue, playback })
    }

    /// Puts back the queue a replace took and returns to what was playing: a track where it was,
    /// paused again if it was, a stopped one seated and stopped. A station comes back paused, since
    /// re-tuning is network work this lock can't wait on, and play reopens it.
    pub fn restore_replaced_queue<R: rand::Rng + ?Sized>(
        &mut self,
        replaced: ReplacedQueue,
        rng: &mut R,
    ) -> Vec<PlayerAction> {
        let ReplacedQueue { queue, playback } = replaced;
        self.queue.restore_replaced(queue, rng);
        let actions = self.resume_held(playback);
        self.close_gap_unless_seated();
        actions
    }

    fn resume_held(&mut self, held: HeldPlayback) -> Vec<PlayerAction> {
        let HeldPlayback { source, status, position_ms, duration_ms } = held;
        match (source, status) {
            (
                Some(PlaybackSource::Track(track)),
                PlaybackStatus::Playing | PlaybackStatus::Paused,
            ) => {
                let mut actions = play_track_inner(self, track, Some(position_ms));
                self.restore_paused(status == PlaybackStatus::Paused, &mut actions);
                actions
            }
            (Some(PlaybackSource::Station(station)), _) => {
                let mut actions = self.build_stop_actions(0);
                actions.extend(restore_station(self, station));
                actions
            }
            (seated, _) => {
                let actions = self.build_stop_actions(0);
                self.source = seated;
                self.position_ms = position_ms;
                self.duration_ms = duration_ms;
                actions
            }
        }
    }
}
