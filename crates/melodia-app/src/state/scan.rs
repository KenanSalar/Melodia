//! The library scan in flight, as everything outside it sees one: how far it has got, and the
//! switch that stops it.

use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Where a folder scan has got to. Only the first two can still be cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPhase {
    /// Walking the folder, before anything is known about what needs reading.
    Discovering,
    Reading,
    /// The last write, which brings the library's counts in line and can't stop part way.
    Finishing,
    /// Cancelled, and still writing what had been read when it was, or taking a cancelled import
    /// back out.
    Stopping,
}

impl ScanPhase {
    pub fn is_cancellable(self) -> bool {
        matches!(self, Self::Discovering | Self::Reading)
    }
}

/// One progress sample. `None` on the channel means no scan is running.
#[derive(Debug, Clone)]
pub struct ScanProgressTick {
    pub phase: ScanPhase,
    /// Audio files the walk has found.
    pub found: u32,
    /// Files read so far, of the `total` that needed reading.
    pub done: u32,
    pub total: u32,
    pub current_file: String,
}

/// The scan-progress channel and the switch every scan stops on.
///
/// A cancel goes through an epoch: each scan takes a child of the current one, and
/// [`cancel`](Self::cancel) cancels it and installs a fresh one. So it stops every scan running
/// at that moment, a boot reconcile and a rescan alike, while one started a moment later starts
/// clean. The epoch descends from the shutdown token, which is how quitting stops a scan at the
/// same checkpoints.
pub struct ScanControl {
    progress: watch::Sender<Option<ScanProgressTick>>,
    epoch: Mutex<CancellationToken>,
    shutdown: CancellationToken,
    /// Moved by [`cancel`](Self::cancel) alone, so a scan can tell the user stopping it from the
    /// app closing, even once both have happened.
    user_cancels: AtomicU64,
}

impl ScanControl {
    pub fn new(shutdown: &CancellationToken) -> Self {
        let (progress, _) = watch::channel(None);
        Self {
            progress,
            epoch: Mutex::new(shutdown.child_token()),
            shutdown: shutdown.clone(),
            user_cancels: AtomicU64::new(0),
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<ScanProgressTick>> {
        self.progress.subscribe()
    }

    /// Returns the token a scan starting now stops on.
    pub fn token(&self) -> CancellationToken {
        self.epoch.lock().child_token()
    }

    /// How many times the user has cancelled. A scan that reads it before taking its token and
    /// again once stopped knows whether one of those cancels was what stopped it.
    pub fn user_cancels(&self) -> u64 {
        self.user_cancels.load(Ordering::Relaxed)
    }

    /// Stops every scan running now, showing it as stopping until it lets go of the bar.
    pub fn cancel(&self) {
        // Relaxed is enough: a scan reads the count only after seeing its token cancelled, and
        // the cancel below orders that after this bump.
        self.user_cancels.fetch_add(1, Ordering::Relaxed);
        let fresh = self.shutdown.child_token();
        std::mem::replace(&mut *self.epoch.lock(), fresh).cancel();
        self.progress.send_if_modified(|tick| match tick {
            Some(tick) if tick.phase.is_cancellable() => {
                tick.phase = ScanPhase::Stopping;
                true
            }
            _ => false,
        });
    }

    /// Shows `tick` unless `token` was cancelled, so a worker's late report can't paint over the
    /// "Stopping" a cancel put up, and answers whether it did. The check runs under the channel's
    /// lock, which `cancel` takes only after cancelling, so the two can't interleave the other way
    /// round.
    pub(crate) fn publish(&self, token: &CancellationToken, tick: ScanProgressTick) -> bool {
        self.progress.send_if_modified(|current| {
            if token.is_cancelled() {
                return false;
            }
            *current = Some(tick);
            true
        })
    }

    pub(crate) fn clear(&self) {
        self.progress.send_replace(None);
    }
}
