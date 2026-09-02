//! Tests for the counters the audio device's error callback writes into.

use std::sync::Arc;

use cpal::{Error, ErrorKind};

use super::{AudioStreamHealth, error_callback};

fn backend(description: &str) -> Error {
    Error::with_message(ErrorKind::BackendError, description.to_owned())
}

#[test]
fn an_idle_stream_reports_nothing() {
    let health = AudioStreamHealth::default();
    assert!(health.drain().is_none());
}

#[test]
fn each_kind_lands_in_its_own_counter() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::Xrun.into());
    health.record(&ErrorKind::Xrun.into());
    health.record(&backend("poll failed"));
    health.record(&ErrorKind::DeviceNotAvailable.into());

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
    health.record(&ErrorKind::Xrun.into());
    health.record(&ErrorKind::StreamInvalidated.into());
    assert!(health.drain().is_some());
    assert!(health.drain().is_none());
}

/// Means the same to a user as an unplugged device: no sound again on its own.
#[test]
fn an_invalidated_stream_reads_as_a_lost_device() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::StreamInvalidated.into());

    let report = health.drain().unwrap_or_default();
    assert!(report.device_lost);
    assert_eq!(report.other, 0);
}

/// `ErrorKind` is `#[non_exhaustive]`, so the arms this tree names are a subset
/// and everything else has to land somewhere countable rather than nowhere.
#[test]
fn a_kind_with_no_arm_of_its_own_still_counts() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::DeviceBusy.into());
    health.record(&ErrorKind::RealtimeDenied.into());

    let report = health.drain().unwrap_or_default();
    assert_eq!(report.other, 2);
    assert!(!report.device_lost);
    assert_eq!(report.underruns, 0);
}

/// A kind carrying no message still has to describe itself, since the count
/// alone says nothing actionable.
#[test]
fn a_kind_without_a_message_still_names_itself() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::PermissionDenied.into());

    let report = health.drain().unwrap_or_default();
    let described = report.first_other_error.unwrap_or_default();
    assert!(!described.is_empty(), "an errorless description tells a reporter nothing");
}

/// Everything after the first is the same fault repeating, and keeping it would
/// trade one allocation for another on the audio thread.
#[test]
fn the_first_unclassified_description_is_the_one_kept() {
    let health = AudioStreamHealth::default();
    health.record(&backend("first"));
    health.record(&backend("second"));

    let report = health.drain().unwrap_or_default();
    assert_eq!(report.other, 2);
    assert_eq!(report.first_other_error.as_deref(), Some("first"));

    // And the slot is empty again, so the next window reports its own.
    health.record(&backend("third"));
    let next = health.drain().unwrap_or_default();
    assert_eq!(next.first_other_error.as_deref(), Some("third"));
}

#[test]
fn a_window_with_no_unclassified_error_carries_no_description() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::Xrun.into());

    let report = health.drain().unwrap_or_default();
    assert_eq!(report.underruns, 1);
    assert!(report.first_other_error.is_none());
}

/// `output::device::open` clones the callback once per configuration its ladder
/// retries, so every clone has to reach the same counters.
#[test]
fn a_cloned_callback_writes_to_the_same_counters() {
    let health = Arc::new(AudioStreamHealth::default());
    let mut callback = error_callback(Arc::clone(&health));
    let mut retry = callback.clone();

    callback(ErrorKind::Xrun.into());
    retry(ErrorKind::Xrun.into());

    assert_eq!(health.drain().unwrap_or_default().underruns, 2);
}
