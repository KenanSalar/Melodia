//! The loudness pass as everything outside it sees one: how far it has got, the tracks the queue
//! wants levelled first, and the switches that stop, resume and wake it.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::watch;

use super::signal::Signal;

/// Where the pass stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MeasurePhase {
    /// Nothing left to measure, or measuring is off.
    #[default]
    Idle,
    Running,
    /// Cut off by a library scan, and picking up once it ends.
    WaitingForScan,
    /// Stopped by the user with work left.
    Stopped,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeasureProgress {
    pub phase: MeasurePhase,
    /// Tracks decoded of the `total` the pass set out to, an album re-read counting every member.
    pub done: u32,
    pub total: u32,
}

/// The pass's progress channel and the controls it answers to.
pub struct LoudnessControl {
    progress: watch::Sender<MeasureProgress>,
    /// The ids of the queue window's tracks that carry no gain, which a pass takes first.
    queued: watch::Sender<Vec<i64>>,
    /// Holds for the session: only [`resume`](Self::resume) lets a stopped pass run again, so the
    /// user decides when it does.
    stopped: AtomicBool,
    kick: Signal,
    gains_measured: Signal,
}

impl LoudnessControl {
    pub fn new() -> Self {
        Self {
            progress: watch::Sender::new(MeasureProgress::default()),
            queued: watch::Sender::new(Vec::new()),
            stopped: AtomicBool::new(false),
            kick: Signal::new(),
            gains_measured: Signal::new(),
        }
    }

    /// Wakes the pass and the queue window right after a live control they read moved: neither
    /// reads a setting from disk, so a kick needn't wait for the persist.
    pub fn kick(&self) {
        self.kick.bump();
    }

    /// Stops the pass, and keeps it stopped through new tracks arriving until [`resume`].
    ///
    /// [`resume`]: Self::resume
    pub fn stop(&self) {
        // Relaxed is enough: `publish` reads the flag under the channel's lock, which is taken
        // below only after this store.
        self.stopped.store(true, Ordering::Relaxed);
        self.progress.send_if_modified(|progress| {
            let measuring =
                matches!(progress.phase, MeasurePhase::Running | MeasurePhase::WaitingForScan);
            if measuring {
                progress.phase = MeasurePhase::Stopped;
            }
            measuring
        });
    }

    pub fn resume(&self) {
        self.stopped.store(false, Ordering::Relaxed);
        self.kick();
    }

    pub fn subscribe_progress(&self) -> watch::Receiver<MeasureProgress> {
        self.progress.subscribe()
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Shows `progress`, except a running one once stopped, so a measurement committing just
    /// after Stop can't paint over it. The check runs under the channel's lock, which [`stop`]
    /// takes only after raising the flag, so the two can't interleave the other way round.
    ///
    /// [`stop`]: Self::stop
    pub(crate) fn publish(&self, progress: MeasureProgress) {
        self.progress.send_if_modified(|current| {
            let painted_over = progress.phase == MeasurePhase::Running && self.is_stopped();
            if painted_over || *current == progress {
                return false;
            }
            *current = progress;
            true
        });
    }

    pub(crate) fn offer_queued(&self, ids: Vec<i64>) {
        self.queued.send_if_modified(|held| {
            if *held == ids {
                return false;
            }
            *held = ids;
            true
        });
    }

    pub(crate) fn subscribe_queued(&self) -> watch::Receiver<Vec<i64>> {
        self.queued.subscribe()
    }

    pub(crate) fn subscribe_kick(&self) -> watch::Receiver<u64> {
        self.kick.subscribe()
    }

    /// Bumped once a pass has committed a job, so a queued track measured mid-pass plays measured.
    pub(crate) fn gains_committed(&self) {
        self.gains_measured.bump();
    }

    pub(crate) fn subscribe_gains_measured(&self) -> watch::Receiver<u64> {
        self.gains_measured.subscribe()
    }
}

impl Default for LoudnessControl {
    fn default() -> Self {
        Self::new()
    }
}
