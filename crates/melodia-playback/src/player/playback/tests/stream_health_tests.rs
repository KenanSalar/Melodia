//! Tests for the counters the audio device's error callback writes into.

use cpal::{Error, ErrorKind};

use super::AudioStreamHealth;

fn backend(description: &str) -> Error {
    Error::with_message(ErrorKind::BackendError, description.to_owned())
}

/// A quiet window still reports, as a zero — what `tasks::audio_health`'s warn
/// latch re-arms on.
#[test]
fn an_idle_stream_reports_nothing() {
    let report = AudioStreamHealth::default().drain();
    assert_eq!(report.underruns, 0);
    assert_eq!(report.other, 0);
    assert!(!report.device_lost);
    assert!(report.first_other_error.is_none());
}

#[test]
fn each_kind_lands_in_its_own_counter() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::Xrun.into());
    health.record(&ErrorKind::Xrun.into());
    health.record(&backend("poll failed"));
    health.record(&ErrorKind::DeviceNotAvailable.into());

    let report = health.drain();
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

    let first = health.drain();
    assert_eq!(first.underruns, 1);
    assert!(first.device_lost);

    let second = health.drain();
    assert_eq!(second.underruns, 0);
    assert!(!second.device_lost);
}

/// Means the same to a user as an unplugged device: no sound again on its own.
#[test]
fn an_invalidated_stream_reads_as_a_lost_device() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::StreamInvalidated.into());

    let report = health.drain();
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

    let report = health.drain();
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

    let report = health.drain();
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

    let report = health.drain();
    assert_eq!(report.other, 2);
    assert_eq!(report.first_other_error.as_deref(), Some("first"));

    // And the slot is empty again, so the next window reports its own.
    health.record(&backend("third"));
    let next = health.drain();
    assert_eq!(next.first_other_error.as_deref(), Some("third"));
}

#[test]
fn a_window_with_no_unclassified_error_carries_no_description() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::Xrun.into());

    let report = health.drain();
    assert_eq!(report.underruns, 1);
    assert!(report.first_other_error.is_none());
}

/// The reopen path takes the loss on its own, and whichever of it and the drain swaps first owns
/// the event: the other must not see the same loss and reopen a second time.
#[test]
fn taking_a_lost_device_takes_it_from_the_drain() {
    let health = AudioStreamHealth::default();
    health.record(&ErrorKind::DeviceNotAvailable.into());
    health.record(&ErrorKind::Xrun.into());

    assert!(health.take_device_lost());
    assert!(!health.take_device_lost(), "a loss is taken once");
    let report = health.drain();
    assert!(!report.device_lost, "the drain saw a loss already taken");
    assert_eq!(report.underruns, 1, "taking the loss must leave the counters to the drain");
}

/// The stall watch compares two reads, so the count is never reset by a drain: resetting it would
/// read as a callback that stopped.
#[test]
fn the_heartbeat_counts_blocks_and_survives_a_drain() {
    let health = AudioStreamHealth::default();
    health.beat();
    health.beat();
    let _ = health.drain();

    assert_eq!(health.blocks(), 2);
}
