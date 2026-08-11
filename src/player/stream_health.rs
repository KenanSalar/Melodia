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
//! `snd_pcm_writei`, and cpal's two `BackendSpecific` sites `continue` inside its
//! worker loop. [`crate::tasks::audio_health`] drains and decides.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rodio::cpal::StreamError;

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
    first_backend_error: parking_lot::Mutex<Option<String>>,
}

/// Everything [`AudioStreamHealth::drain`] found.
#[derive(Debug, Default)]
pub struct StreamHealthReport {
    /// Buffer under/overruns. cpal recovers from these itself.
    pub underruns: u64,
    /// Backend errors that are neither an xrun nor a lost device.
    pub other: u64,
    /// The description of the first `other`, when one was captured.
    pub first_backend_error: Option<String>,
    /// The device went away, or its configuration stopped being valid.
    pub device_lost: bool,
}

impl AudioStreamHealth {
    /// Record one stream error. No blocking lock, no formatting, no I/O.
    ///
    /// By value because that is how the callback receives it, so the
    /// `BackendSpecific` description is moved into the slot rather than cloned.
    pub fn record(&self, err: StreamError) {
        match err {
            StreamError::BufferUnderrun => {
                self.underruns.fetch_add(1, Ordering::Relaxed);
            }
            // Both mean the stream won't produce sound again on its own and both
            // reach the user the same way, so `StreamInvalidated` earns no
            // counter of its own.
            StreamError::DeviceNotAvailable | StreamError::StreamInvalidated => {
                self.device_lost.store(true, Ordering::Relaxed);
            }
            StreamError::BackendSpecific { err } => {
                self.other.fetch_add(1, Ordering::Relaxed);
                // First of the window only: `try_lock` because a blocking one
                // here is what this module exists to avoid, and only-if-empty so
                // a spin frees a short string rather than trading one for another.
                if let Some(mut slot) = self.first_backend_error.try_lock()
                    && slot.is_none()
                {
                    *slot = Some(err.description);
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
            first_backend_error: self.first_backend_error.lock().take(),
            device_lost,
        })
    }
}

/// The callback to hand `DeviceSinkBuilder::with_error_callback`.
///
/// `Clone` because `open_sink_or_fallback` clones it once per configuration it
/// retries; the captured `Arc` is what supplies that.
pub fn error_callback(
    health: Arc<AudioStreamHealth>,
) -> impl FnMut(StreamError) + Clone + Send + 'static {
    move |err| health.record(err)
}

#[cfg(test)]
#[path = "tests/stream_health_tests.rs"]
mod tests;
