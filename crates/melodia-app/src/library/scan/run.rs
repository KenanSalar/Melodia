//! One folder scan's side of [`ScanControl`]: the token it stops on and the progress it shows.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, ScanControl, ScanPhase, ScanProgressTick};
use melodia_store::media::ingest::scanner::ScanObserver;

/// Floor between two progress ticks. A fast SSD scan reaches the scanner's every-10-files gate
/// far faster than the UI can paint, and each tick allocates the file name it carries.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// One folder scan in flight, shared with the blocking threads that walk and read for it. The
/// bar clears when the last of them lets go, whichever way the scan ended.
pub(super) struct ScanRun {
    control: Arc<ScanControl>,
    cancel: CancellationToken,
    found_count: AtomicU32,
    /// What the incremental filter left to read.
    total: AtomicU32,
    /// Files read by the chunks before the current one, so a tick counts against the whole scan.
    read_before: AtomicU32,
    last_tick: Mutex<Instant>,
}

impl ScanRun {
    /// Starts a run and puts the bar up at once, ahead of the walk.
    pub(super) fn start(state: &AppState, cancel: CancellationToken) -> Arc<Self> {
        let run = Arc::new(Self {
            control: Arc::clone(&state.scan),
            cancel,
            found_count: AtomicU32::new(0),
            total: AtomicU32::new(0),
            read_before: AtomicU32::new(0),
            last_tick: Mutex::new(Instant::now()),
        });
        run.publish(ScanPhase::Discovering, 0, "");
        run
    }

    pub(super) fn begin_reading(&self, total: u32) {
        self.total.store(total, Ordering::Relaxed);
        self.publish(ScanPhase::Reading, 0, "");
    }

    pub(super) fn chunk_read(&self, files: u32) {
        self.read_before.fetch_add(files, Ordering::Relaxed);
    }

    /// Shows the last write as under way unless the scan was cancelled first, and answers whether
    /// it did. Refusing under the channel's lock is what stops a cancel that lands after the chunk
    /// loop's last check from being ignored.
    pub(super) fn try_begin_finishing(&self) -> bool {
        self.publish(ScanPhase::Finishing, self.total.load(Ordering::Relaxed), "")
    }

    /// Restarts [`PROGRESS_INTERVAL`] and answers `true` if it had run out, `false` if the tick
    /// asking should be dropped.
    fn claim_tick(&self) -> bool {
        let mut last = self.last_tick.lock();
        if last.elapsed() < PROGRESS_INTERVAL {
            return false;
        }
        *last = Instant::now();
        true
    }

    fn publish(&self, phase: ScanPhase, done: u32, current_file: &str) -> bool {
        self.control.publish(
            &self.cancel,
            ScanProgressTick {
                phase,
                found: self.found_count.load(Ordering::Relaxed),
                done,
                total: self.total.load(Ordering::Relaxed),
                current_file: current_file.to_owned(),
            },
        )
    }
}

impl ScanObserver for ScanRun {
    fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn found(&self, count: u32) {
        self.found_count.store(count, Ordering::Relaxed);
        if self.claim_tick() {
            self.publish(ScanPhase::Discovering, 0, "");
        }
    }

    /// The last file always lands, so the bar reaches the end rather than stopping short of it.
    fn read(&self, done: u32, file_name: &str) {
        let done = self.read_before.load(Ordering::Relaxed) + done;
        if done == self.total.load(Ordering::Relaxed) || self.claim_tick() {
            self.publish(ScanPhase::Reading, done, file_name);
        }
    }
}

impl Drop for ScanRun {
    fn drop(&mut self) {
        self.control.clear();
    }
}
