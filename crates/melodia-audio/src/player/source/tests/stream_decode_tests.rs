//! What a live mount's reader has to refuse, and that a decoder still comes out of it.
//!
//! Driven over an in-memory cursor rather than a socket: the two answers under test are the ones
//! `LiveSource` gives regardless of what is behind it, and the open is the same call the feed
//! thread makes once its ring has bytes.

use std::io::Cursor;

use super::{LiveSource, StreamDecoder};
use melodia_core::error::AppError;
use melodia_testkit::ASSETS_DIR;

fn mount(bytes: Vec<u8>) -> Box<dyn symphonia::core::io::MediaSource> {
    Box::new(LiveSource(Cursor::new(bytes)))
}

fn fixture(name: &str) -> Result<Vec<u8>, AppError> {
    Ok(std::fs::read(std::path::Path::new(ASSETS_DIR).join(name))?)
}

/// Both answers are the shape of a live mount rather than a shortcoming, and the length is the
/// one that bites: a stated length sends the probe hunting for trailing metadata a stream never
/// sends, and it waits for it against a socket that stays open.
#[test]
fn a_live_mount_states_no_length_and_refuses_to_seek() {
    let source = LiveSource(Cursor::new(vec![0_u8; 64]));

    assert!(!symphonia::core::io::MediaSource::is_seekable(&source));
    assert_eq!(symphonia::core::io::MediaSource::byte_len(&source), None);
}

/// The open is where a mount's shape is pinned, since a renegotiation mid-stream ends the source
/// instead. The deck builds its converter from exactly this answer.
#[test]
fn an_opened_mount_reports_the_shape_the_deck_converts_from() -> Result<(), AppError> {
    let mut decoder = StreamDecoder::open(mount(fixture("silence.mp3")?), Some("audio/mpeg"))?;

    // The fixture's own shape, which is the point: the deck converts from what the mount
    // turned out to be rather than from anything the caller assumed.
    let shape = decoder.shape();
    assert_eq!(shape.channels.get(), 1);
    assert_eq!(shape.rate.get(), 44_100);
    assert!(decoder.next().is_some(), "an opened mount hands out samples");
    Ok(())
}

/// A mount serving something that is not audio fails at the open, where the station can be
/// reported as unplayable, rather than by handing the deck a source with nothing in it.
#[test]
fn a_mount_serving_something_else_fails_the_open() {
    let opened = StreamDecoder::open(mount(b"<html>not audio</html>".to_vec()), Some("text/html"));

    assert!(matches!(opened, Err(AppError::Player(_))));
}
