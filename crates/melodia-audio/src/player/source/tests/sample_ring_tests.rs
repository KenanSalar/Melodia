//! Tests for the ring a station and a file both play through: the whole-frame gate, the cursors,
//! and the writer's two ways out of a full ring.

use std::sync::mpsc;
use std::time::Duration;

use super::{FeedState, Pulled, RingReader, RingWriter, ring};
use crate::player::source::tests::helpers::shape;

/// Low enough that a ring is a handful of slots: [`DEPTH`] of it is [`DEPTH_SAMPLES`] in mono.
const RATE: u32 = 1_000;
const DEPTH: Duration = Duration::from_millis(10);
const DEPTH_SAMPLES: usize = 10;

/// Short, so a writer parked on a full ring sees the reader go within the test's lifetime.
const PARK: Duration = Duration::from_millis(1);

/// How long a writer gets to notice the reader went, so one that never does fails rather than
/// hangs the suite.
const PATIENCE: Duration = Duration::from_secs(5);

fn ring_of(channels: u16) -> (RingWriter, RingReader) {
    ring(shape(channels, RATE), DEPTH, PARK, FeedState::new())
}

/// A distinct, exactly representable sample per index, without a lossy cast.
fn sample_at(index: u32) -> f32 {
    f32::from(u16::try_from(index % 251).unwrap_or(0))
}

#[test]
fn a_finished_and_empty_ring_ends() {
    let (writer, mut reader) = ring_of(2);
    writer.finish();

    assert_eq!(reader.pull(), Pulled::Ended);
}

/// A trailing half frame is dropped rather than handed out: half a frame flips the deck's channel
/// parity for everything that plays after it, and it is inaudible either way.
#[test]
fn a_trailing_partial_frame_is_dropped_rather_than_played() {
    let (writer, mut reader) = ring_of(2);
    assert!(writer.push(0.5));
    writer.finish();

    assert_eq!(reader.pull(), Pulled::Ended);
}

/// While the feed is still live, a partial frame is starved for the *whole* frame rather than one
/// real sample followed by one owed as silence.
#[test]
fn a_partial_frame_is_never_split_across_the_ring_and_silence() {
    let (writer, mut reader) = ring_of(2);
    assert!(writer.push(0.5));

    assert_eq!(reader.pull(), Pulled::Starved);
    assert_eq!(reader.pull(), Pulled::Starved);
    // The sample that was there all along is only taken once its frame partner arrives.
    assert!(writer.push(0.75));
    assert_eq!(reader.pull(), Pulled::Sample(0.5));
    assert_eq!(reader.pull(), Pulled::Sample(0.75));
}

#[test]
fn the_ring_wraps_without_losing_order() {
    let (writer, mut reader) = ring_of(1);

    // Many times the depth, so the cursors wrap again and again.
    for i in 0..1_000_u32 {
        assert!(writer.push(sample_at(i)));
        assert_eq!(reader.pull(), Pulled::Sample(sample_at(i)));
    }
}

/// A file's first fill decodes on the caller's thread with nothing pulling, and stops on this:
/// reading full a sample late sends it into a push that waits for room no one will make.
#[test]
fn a_writer_reads_full_once_the_ring_holds_its_depth() {
    let (writer, _reader) = ring_of(1);
    for _ in 1..DEPTH_SAMPLES {
        assert!(writer.push(0.0));
    }
    let one_short = writer.is_full();

    assert!(writer.push(0.0));

    assert_eq!((one_short, writer.is_full()), (false, true), "(one sample short, at depth)");
}

/// The feeder parks on a full ring, so only the reader letting go releases it. Without that,
/// closing a station would leak a thread and its socket, and a file its handle.
#[test]
fn a_blocked_writer_gives_up_once_the_reader_is_dropped() {
    let (writer, reader) = ring_of(1);
    let (gave_up, feeder_done) = mpsc::channel();
    std::thread::spawn(move || {
        let mut pushed = 0_usize;
        while writer.push(1.0) {
            pushed += 1;
        }
        let _ = gave_up.send(pushed);
    });

    drop(reader);

    assert_eq!(feeder_done.recv_timeout(PATIENCE), Ok(DEPTH_SAMPLES));
}
