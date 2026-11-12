//! The hand-off that carries a decoder on from one cue track into the next of its file: who gets
//! the decoder, standing where, and when nobody does.

use std::sync::Arc;

use tempfile::TempDir;

use super::*;
use crate::player::source::tests::helpers::{frame_indexed_wav, is_frame};
use melodia_core::entities::cue::{CueFrames, CueSpan};
use melodia_core::error::AppError;

/// Track `start..end` of a frame-indexed image at `path`, in cue frames of 640 samples.
fn cue(path: &str, start: u32, end: Option<u32>) -> TrackAudio {
    let frames = |count: u32| CueFrames::try_from(i64::from(count)).ok();
    let span = frames(start).and_then(|start| CueSpan::new(start, end.and_then(frames)).ok());
    TrackAudio { path: path.to_owned(), span }
}

fn image(tmp: &TempDir) -> Result<String, AppError> {
    let path = tmp.path().join("image.wav");
    frame_indexed_wav(&path, 30_000)?;
    Ok(path.to_string_lossy().into_owned())
}

fn park_first_track(slot: &Arc<ParkSlot>, path: &str) -> Result<(), AppError> {
    let first = cue(path, 0, Some(15));
    let ticket =
        slot.ticket(&first).ok_or_else(|| AppError::player_msg("track 1 ends inside the file"))?;
    let mut decoder = FileDecoder::open_track(&first)?;
    decoder.by_ref().for_each(drop);
    ticket.park(decoder);
    Ok(())
}

#[test]
fn a_track_playing_on_to_the_end_of_its_file_gets_no_ticket() {
    let slot = Arc::new(ParkSlot::default());
    assert!(slot.ticket(&cue("/music/image.flac", 15, None)).is_none());
}

#[test]
fn the_next_track_of_the_file_takes_the_decoder_where_it_stopped() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = image(&tmp)?;
    let slot = Arc::new(ParkSlot::default());
    park_first_track(&slot, &path)?;

    let taken = slot.take(&cue(&path, 15, Some(30)));

    let samples: Vec<f32> = taken.into_iter().flatten().collect();
    let shape = (samples.len(), is_frame(samples.first(), 9_600), is_frame(samples.last(), 19_199));
    assert_eq!(shape, (9_600, true, true), "{:?}..{:?}", samples.first(), samples.last());
    Ok(())
}

#[test]
fn a_decoder_goes_only_to_a_track_of_its_file_starting_where_it_stopped() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = image(&tmp)?;
    let rows = [
        ("another file", cue("/music/other.wav", 15, Some(30))),
        ("a later start", cue(&path, 16, Some(30))),
        ("the whole file", TrackAudio { path: path.clone(), span: None }),
    ];
    for (label, next) in rows {
        let slot = Arc::new(ParkSlot::default());
        park_first_track(&slot, &path)?;
        assert!(slot.take(&next).is_none(), "{label}");
    }
    Ok(())
}

#[test]
fn a_take_empties_the_slot_whoever_asked() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = image(&tmp)?;
    let slot = Arc::new(ParkSlot::default());
    park_first_track(&slot, &path)?;

    let _wrong = slot.take(&cue(&path, 16, Some(30)));

    assert!(slot.take(&cue(&path, 15, Some(30))).is_none());
    Ok(())
}

#[test]
fn a_ticket_handed_out_before_a_clear_parks_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = image(&tmp)?;
    let slot = Arc::new(ParkSlot::default());
    let first = cue(&path, 0, Some(15));
    let ticket =
        slot.ticket(&first).ok_or_else(|| AppError::player_msg("track 1 ends inside the file"))?;
    let mut decoder = FileDecoder::open_track(&first)?;
    decoder.by_ref().for_each(drop);

    slot.clear();
    ticket.park(decoder);

    assert!(slot.take(&cue(&path, 15, Some(30))).is_none());
    Ok(())
}

#[test]
fn a_decoder_that_did_not_play_its_span_out_is_not_parked() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = image(&tmp)?;
    let slot = Arc::new(ParkSlot::default());
    let first = cue(&path, 0, Some(15));
    let ticket =
        slot.ticket(&first).ok_or_else(|| AppError::player_msg("track 1 ends inside the file"))?;
    let mut decoder = FileDecoder::open_track(&first)?;
    decoder.by_ref().take(100).for_each(drop);

    ticket.park(decoder);

    assert!(slot.take(&cue(&path, 15, Some(30))).is_none());
    Ok(())
}
