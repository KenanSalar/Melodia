//! What the device callbacks may do: record into atomics and return. cpal calls the error callback
//! on its worker thread inside the xrun handler, where a log line is a synchronous write on the
//! thread whose missed deadline caused the xrun, and every arm is a storm vector, so none logs.
//! `tasks::audio_health` drains and decides.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::{Error, ErrorKind};

/// `Relaxed` throughout: each field stands alone, and the drain only needs the counts eventually.
#[derive(Debug, Default)]
pub struct AudioStreamHealth {
    underruns: AtomicU64,
    other: AtomicU64,
    device_lost: AtomicBool,
    /// Blocks the data callback has been asked for, ever. Never drained: only its movement means
    /// anything, which is why a reader compares two reads rather than taking a count.
    blocks: AtomicU64,
    /// Kept because the counter alone says nothing actionable.
    first_other_error: parking_lot::Mutex<Option<String>>,
}

/// Everything [`AudioStreamHealth::drain`] found.
#[derive(Debug, Default)]
pub struct StreamHealthReport {
    /// Buffer under/overruns. cpal recovers from these itself.
    pub underruns: u64,
    /// Neither an xrun nor a lost device, `BackendError` being one kind of several.
    pub other: u64,
    /// The description of the first `other`, when one was captured.
    pub first_other_error: Option<String>,
    /// The device went away, or its configuration stopped being valid.
    pub device_lost: bool,
}

impl AudioStreamHealth {
    /// No blocking lock and no I/O; the catch-all formats once a window.
    pub fn record(&self, err: &Error) {
        match err.kind() {
            ErrorKind::Xrun => self.record_xrun(),
            // Neither will sound again on its own, and both reach the user alike.
            ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated => {
                self.report_device_lost();
            }
            // `#[non_exhaustive]`, so a kind added upstream still counts.
            _ => {
                self.other.fetch_add(1, Ordering::Relaxed);
                // `try_lock` and only-if-empty, so a storm formats one short string.
                if let Some(mut slot) = self.first_other_error.try_lock()
                    && slot.is_none()
                {
                    *slot = Some(err.to_string());
                }
            }
        }
    }

    pub fn record_xrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Also where a backend with no cpal error reports, so every output shares one recovery path.
    pub fn report_device_lost(&self) {
        self.device_lost.store(true, Ordering::Relaxed);
    }

    /// For the loss no host reports: `PipeWire`'s ALSA plugin raises no `POLLHUP` when its server
    /// goes, and the callback simply stops being called.
    pub fn beat(&self) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
    }

    /// A value that stops moving is a stalled stream.
    pub fn blocks(&self) -> u64 {
        self.blocks.load(Ordering::Relaxed)
    }

    /// For a reopen, which wants the loss sooner than a drain window; the swap means one taker
    /// acts on it.
    pub fn take_device_lost(&self) -> bool {
        self.device_lost.swap(false, Ordering::Relaxed)
    }

    /// A quiet window is a zeroed report rather than `None`, since `tasks::audio_health`'s warn
    /// latch re-arms on the zero.
    pub fn drain(&self) -> StreamHealthReport {
        let underruns = self.underruns.swap(0, Ordering::Relaxed);
        let other = self.other.swap(0, Ordering::Relaxed);
        let device_lost = self.device_lost.swap(false, Ordering::Relaxed);
        if underruns == 0 && other == 0 && !device_lost {
            return StreamHealthReport::default();
        }
        StreamHealthReport {
            underruns,
            other,
            first_other_error: self.first_other_error.lock().take(),
            device_lost,
        }
    }
}

/// The error callback `output::device` hands every stream it builds.
pub fn error_callback(health: Arc<AudioStreamHealth>) -> impl FnMut(Error) + Send + 'static {
    move |err| health.record(&err)
}

#[cfg(test)]
#[path = "tests/stream_health_tests.rs"]
mod tests;
