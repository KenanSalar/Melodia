//! Tests for the counters the audio device's error callback writes into.

use std::sync::Arc;

use cpal::{BackendSpecificError, StreamError};

use super::{AudioStreamHealth, error_callback};

fn backend(description: &str) -> StreamError {
    StreamError::BackendSpecific {
        err: BackendSpecificError {
            description: description.to_owned(),
        },
    }
}

#[test]
fn an_idle_stream_reports_nothing() {
    let health = AudioStreamHealth::default();
    assert!(health.drain().is_none());
}

#[test]
fn each_variant_lands_in_its_own_counter() {
    let health = AudioStreamHealth::default();
    health.record(StreamError::BufferUnderrun);
    health.record(StreamError::BufferUnderrun);
    health.record(backend("poll failed"));
    health.record(StreamError::DeviceNotAvailable);

    let report = health.drain().unwrap_or_default();
    assert_eq!(report.underruns, 2);
    assert_eq!(report.other, 1);
    assert!(report.device_lost);
}

/// Keeps a `debug` line a rate rather than a running total, and stops one
/// disconnect toasting on every tick after it.
#[test]
fn a_drain_takes_what_it_reports() {
    let health = AudioStreamHealth::default();
    health.record(StreamError::BufferUnderrun);
    health.record(StreamError::StreamInvalidated);
    assert!(health.drain().is_some());
    assert!(health.drain().is_none());
}

/// Means the same to a user as an unplugged device: no sound again on its own.
#[test]
fn an_invalidated_stream_reads_as_a_lost_device() {
    let health = AudioStreamHealth::default();
    health.record(StreamError::StreamInvalidated);

    let report = health.drain().unwrap_or_default();
    assert!(report.device_lost);
    assert_eq!(report.other, 0);
}

/// cpal's two `BackendSpecific` sites sit inside its worker loop, so everything
/// after the first is the same fault repeating — and keeping it would trade one
/// allocation for another on the audio thread.
#[test]
fn the_first_backend_description_is_the_one_kept() {
    let health = AudioStreamHealth::default();
    health.record(backend("first"));
    health.record(backend("second"));

    let report = health.drain().unwrap_or_default();
    assert_eq!(report.other, 2);
    assert_eq!(report.first_backend_error.as_deref(), Some("first"));

    // And the slot is empty again, so the next window reports its own.
    health.record(backend("third"));
    let next = health.drain().unwrap_or_default();
    assert_eq!(next.first_backend_error.as_deref(), Some("third"));
}

#[test]
fn a_window_with_no_backend_error_carries_no_description() {
    let health = AudioStreamHealth::default();
    health.record(StreamError::BufferUnderrun);

    let report = health.drain().unwrap_or_default();
    assert_eq!(report.underruns, 1);
    assert!(report.first_backend_error.is_none());
}

/// `output::device::open` clones the callback once per configuration its ladder
/// retries, so every clone has to reach the same counters.
#[test]
fn a_cloned_callback_writes_to_the_same_counters() {
    let health = Arc::new(AudioStreamHealth::default());
    let mut callback = error_callback(Arc::clone(&health));
    let mut retry = callback.clone();

    callback(StreamError::BufferUnderrun);
    retry(StreamError::BufferUnderrun);

    assert_eq!(health.drain().unwrap_or_default().underruns, 2);
}
