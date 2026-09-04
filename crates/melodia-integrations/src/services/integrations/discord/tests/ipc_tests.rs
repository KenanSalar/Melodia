use std::io::{Cursor, ErrorKind};

use super::{MAX_FRAME_LEN, read_frame, write_frame};

#[test]
fn frame_round_trips() {
    let mut buf: Vec<u8> = Vec::new();
    assert!(write_frame(&mut buf, 1, b"hello world").is_ok());

    let mut cursor = Cursor::new(buf);
    assert_eq!(read_frame(&mut cursor).ok(), Some((1u32, b"hello world".to_vec())));
}

#[test]
fn empty_body_round_trips() {
    let mut buf: Vec<u8> = Vec::new();
    assert!(write_frame(&mut buf, 0, b"").is_ok());

    let mut cursor = Cursor::new(buf);
    assert_eq!(read_frame(&mut cursor).ok(), Some((0u32, Vec::new())));
}

#[test]
fn oversized_declared_length_is_rejected() {
    // A header claiming a body far larger than the cap, with no body bytes —
    // read_frame must reject on the length alone rather than allocate.
    let mut frame: Vec<u8> = Vec::new();
    frame.extend_from_slice(&1u32.to_le_bytes());
    frame.extend_from_slice(&(MAX_FRAME_LEN + 1).to_le_bytes());

    let mut cursor = Cursor::new(frame);
    let kind = read_frame(&mut cursor).err().map(|e| e.kind());
    assert_eq!(kind, Some(ErrorKind::InvalidData));
}

#[test]
fn truncated_header_errors() {
    // Fewer than the 8 header bytes → read_exact fails.
    let mut cursor = Cursor::new(vec![1u8, 0, 0]);
    assert!(read_frame(&mut cursor).is_err());
}
