//! What the audio device's callbacks are allowed to do: record into atomics
//! and return. The data callback's one entry is [`AudioStreamHealth::beat`]; the
//! rest is the error callback's.
//!
//! cpal invokes it on the output worker thread, inside the ALSA xrun handler and
//! *before* its own `try_recover`. It was a `log::warn!`, which under the file
//! sink's `WriteMode::Direct` and `Duplicate::All` is a synchronous file and
//! stderr write holding `flexi_logger`'s lock — on the thread whose missed
//! deadline caused the xrun it was reporting.
//!
//! **Every arm is a storm vector, hence no logging at all**: an xrun per failed
//! `snd_pcm_writei`, and an unclassified error `continue`s inside cpal's worker
//! loop. `tasks::audio_health` drains and decides.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::{Error, ErrorKind};

/// Stream faults recorded since the last drain.
///
/// `Relaxed` throughout: each field stands alone and nothing is published
/// alongside it, so the drain only needs the counts to arrive eventually.
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
    /// Stream errors that are neither an xrun nor a lost device. `ErrorKind::BackendError` is one
    /// of the several kinds that land here, which is why neither this nor the field below is named
    /// after it.
    pub other: u64,
    /// The description of the first `other`, when one was captured.
    pub first_other_error: Option<String>,
    /// The device went away, or its configuration stopped being valid.
    pub device_lost: bool,
}

impl AudioStreamHealth {
    /// Record one stream error. No blocking lock, no I/O.
    ///
    /// The catch-all arm does format, once per window: both gates below have to pass, so a storm
    /// pays for one short string and then nothing.
    pub fn record(&self, err: &Error) {
        match err.kind() {
            ErrorKind::Xrun => {
                self.underruns.fetch_add(1, Ordering::Relaxed);
            }
            // Both mean the stream won't produce sound again on its own and both
            // reach the user the same way, so `StreamInvalidated` earns no
            // counter of its own.
            ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated => {
                self.device_lost.store(true, Ordering::Relaxed);
            }
            // `ErrorKind` is `#[non_exhaustive]`, so this is a catch-all rather
            // than the rest of the variants spelled out: a kind added upstream
            // that this tree has no answer for still belongs in the count.
            _ => {
                self.other.fetch_add(1, Ordering::Relaxed);
                // First of the window only: `try_lock` because a blocking one
                // here is what this module exists to avoid, and only-if-empty so
                // a spin frees a short string rather than trading one for another.
                // The `to_string` is inside both gates for the same reason.
                if let Some(mut slot) = self.first_other_error.try_lock()
                    && slot.is_none()
                {
                    *slot = Some(err.to_string());
                }
            }
        }
    }

    /// Record one data-callback block.
    ///
    /// A loss the host reports arrives through [`Self::record`]. This is for the one it doesn't:
    /// the `PipeWire` ALSA plugin, whose server went away, raises no `POLLHUP`, so cpal's worker
    /// polls on with no timeout and the callback simply stops being called.
    pub fn beat(&self) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
    }

    /// Blocks recorded by [`Self::beat`] so far; a value that stops moving is a stalled stream.
    pub fn blocks(&self) -> u64 {
        self.blocks.load(Ordering::Relaxed)
    }

    /// Take a lost-device report on its own, leaving the counters for [`Self::drain`].
    ///
    /// A reopen wants the loss sooner than a drain window, and a swap means whichever of the two
    /// takes it first is the one that acts on it.
    pub fn take_device_lost(&self) -> bool {
        self.device_lost.swap(false, Ordering::Relaxed)
    }

    /// Take everything recorded since the last call.
    ///
    /// A quiet window is a zeroed report rather than an absence, and that is the whole of why
    /// this returns no `Option`: `tasks::audio_health`'s warn latch re-arms on the zero,
    /// so a caller that could skip an empty window would leave the first fault of a session the
    /// only one ever warned about. The early return keeps the lock off that path either way.
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
