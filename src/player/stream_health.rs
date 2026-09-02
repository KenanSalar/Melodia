//! What the audio device's error callback is allowed to do: record into atomics
//! and return.
//!
//! cpal invokes it on the output worker thread, inside the ALSA xrun handler and
//! *before* its own `try_recover`. It was a `log::warn!`, which under the file
//! sink's `WriteMode::Direct` and `Duplicate::All` is a synchronous file and
//! stderr write holding `flexi_logger`'s lock — on the thread whose missed
//! deadline caused the xrun it was reporting.
//!
//! **Every arm is a storm vector, hence no logging at all**: an xrun per failed
//! `snd_pcm_writei`, and an unclassified error `continue`s inside cpal's worker
//! loop. [`crate::tasks::audio_health`] drains and decides.

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

    /// Take everything recorded since the last call, or `None` if nothing was.
    pub fn drain(&self) -> Option<StreamHealthReport> {
        let underruns = self.underruns.swap(0, Ordering::Relaxed);
        let other = self.other.swap(0, Ordering::Relaxed);
        let device_lost = self.device_lost.swap(false, Ordering::Relaxed);
        if underruns == 0 && other == 0 && !device_lost {
            return None;
        }
        Some(StreamHealthReport {
            underruns,
            other,
            first_other_error: self.first_other_error.lock().take(),
            device_lost,
        })
    }
}

/// The callback to hand `output::device::open`.
///
/// `Clone` because that ladder clones it once per configuration it retries; the
/// captured `Arc` is what supplies that.
pub fn error_callback(
    health: Arc<AudioStreamHealth>,
) -> impl FnMut(Error) + Clone + Send + 'static {
    move |err| health.record(&err)
}

#[cfg(test)]
#[path = "tests/stream_health_tests.rs"]
mod tests;
