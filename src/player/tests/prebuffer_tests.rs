use std::sync::Arc;

use crate::player::audio::AudioSource;
use crate::player::prebuffer::{PrebufferSource, RingWriter, StreamShared};
use crate::player::tests::helpers::shape;

const RATE: u32 = 48_000;

fn stereo() -> (PrebufferSource, RingWriter, Arc<StreamShared>) {
    let shared = StreamShared::new();
    let (source, writer) = PrebufferSource::new(shared.clone(), shape(2, RATE));
    (source, writer, shared)
}

/// A distinct, exactly representable sample per index, without a lossy cast.
fn sample_at(index: u32) -> f32 {
    f32::from(u16::try_from(index % 251).unwrap_or(0))
}

/// Silence, never `None`: ending on starvation would drain the deck and read as `EndOfStream`,
/// which the monitor would take for the station having finished.
#[test]
fn a_dry_ring_yields_silence_rather_than_ending() {
    let (mut source, _writer, shared) = stereo();

    assert_eq!(source.next(), Some(0.0));
    assert_eq!(source.next(), Some(0.0));
    assert!(shared.is_buffering());
}

#[test]
fn buffering_clears_once_the_ring_refills() {
    let (mut source, writer, shared) = stereo();

    assert_eq!(source.next(), Some(0.0));
    assert!(shared.is_buffering());

    // Land on a frame boundary before feeding, so the next decision is taken fresh.
    assert_eq!(source.next(), Some(0.0));
    assert!(writer.push(0.25));
    assert!(writer.push(-0.25));

    assert_eq!(source.next(), Some(0.25));
    assert!(!shared.is_buffering());
}

#[test]
fn a_finished_and_empty_stream_ends() {
    let (mut source, _writer, shared) = stereo();
    shared.finish();

    assert_eq!(source.next(), None);
}

#[test]
fn a_finished_stream_still_drains_what_is_left() {
    let (mut source, writer, shared) = stereo();
    assert!(writer.push(0.5));
    assert!(writer.push(0.75));
    shared.finish();

    assert_eq!(source.next(), Some(0.5));
    assert_eq!(source.next(), Some(0.75));
    assert_eq!(source.next(), None);
}

/// A trailing half frame is dropped rather than handed out: half a frame flips the deck's channel
/// parity for everything that plays after it, and it is inaudible either way.
#[test]
fn a_trailing_partial_frame_is_dropped_rather_than_played() {
    let (mut source, writer, shared) = stereo();
    assert!(writer.push(0.5));
    shared.finish();

    assert_eq!(source.next(), None);
}

/// While the stream is still live, a partial frame is silence for the *whole* frame rather than
/// one real sample followed by one silent one.
#[test]
fn a_partial_frame_is_never_split_across_the_ring_and_silence() {
    let (mut source, writer, _shared) = stereo();
    assert!(writer.push(0.5));

    assert_eq!(source.next(), Some(0.0));
    assert_eq!(source.next(), Some(0.0));
    // The sample that was there all along is only taken once its frame partner arrives.
    assert!(writer.push(0.75));
    assert_eq!(source.next(), Some(0.5));
    assert_eq!(source.next(), Some(0.75));
}

#[test]
fn the_ring_wraps_without_losing_order() {
    let shared = StreamShared::new();
    let (mut source, writer) = PrebufferSource::new(shared, shape(1, RATE));

    // Well past the capacity the configured prebuffer window buys, so the cursors wrap repeatedly.
    for i in 0..500_000_u32 {
        assert!(writer.push(sample_at(i)));
        assert_eq!(source.next(), Some(sample_at(i)));
    }
}

/// The feed thread parks on a full ring, so the only thing that ever releases it is the source
/// going away. Without that, closing a station would leak a thread and its socket.
#[test]
fn a_blocked_writer_gives_up_once_the_source_is_dropped() {
    let shared = StreamShared::new();
    let (source, writer) = PrebufferSource::new(shared, shape(1, RATE));

    let feeder = std::thread::spawn(move || {
        let mut pushed = 0_u64;
        while writer.push(1.0) {
            pushed += 1;
        }
        pushed
    });

    drop(source);
    assert!(feeder.join().is_ok_and(|pushed| pushed > 0));
}

#[test]
fn dropping_the_source_tells_the_feed_thread_to_stop() {
    let (source, _writer, shared) = stereo();
    assert!(!shared.is_abandoned());

    drop(source);
    assert!(shared.is_abandoned());
}

#[test]
fn a_live_stream_reports_no_length_and_refuses_to_seek() {
    let (mut source, _writer, _shared) = stereo();

    assert_eq!(source.total_duration(), None);
    assert_eq!(source.shape(), shape(2, RATE));
    assert!(source.try_seek(std::time::Duration::from_secs(1)).is_err());
}

#[test]
fn the_title_generation_moves_only_when_a_title_is_published() {
    let shared = StreamShared::new();
    let before = shared.title_generation();

    assert_eq!(shared.title(), None);
    assert_eq!(shared.title_generation(), before);

    shared.set_title(Some("Artist - Track".to_owned()));
    assert_eq!(shared.title().as_deref(), Some("Artist - Track"));
    assert!(shared.title_generation() > before);
}
