use std::sync::Arc;

use crate::player::source::audio::{AudioSource, SourceFormat};
use crate::player::source::prebuffer::{PrebufferSource, StreamShared};
use crate::player::source::sample_ring::RingWriter;
use crate::player::source::tests::helpers::shape;

const RATE: u32 = 48_000;

fn stereo() -> (PrebufferSource, RingWriter, Arc<StreamShared>) {
    let shared = StreamShared::new();
    let (source, writer) = PrebufferSource::new(shared.clone(), shape(2, RATE), SourceFormat::F32);
    (source, writer, shared)
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

/// Finished through the station's own cell, which has to be the feed its ring reads.
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
    assert_eq!(shared.title(), None, "queued, not yet heard");
    assert_eq!(shared.title_generation(), before);

    // With no ring yet, its audio starts at the ring's first sample, which counts as heard.
    shared.publish_heard_title();
    assert_eq!(shared.title().as_deref(), Some("Artist - Track"));
    assert!(shared.title_generation() > before);
}
