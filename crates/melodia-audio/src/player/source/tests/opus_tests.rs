//! Tests for the Opus decoder.
//!
//! Counts rather than samples: the audio either side of a pre-skip or end-trim regression is still
//! the right audio, so a comparison against a reference decode would agree on everything it looked
//! at and disagree only on how much there was. The last two ask that of the demuxer's timeline
//! instead, a stated length and a seek's landing against what the decoder hands over.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::player::source::audio::AudioSource;
use crate::player::source::file_decode::FileDecoder;
use melodia_core::error::AppError;
use melodia_testkit::ASSETS_DIR;

fn asset(name: &str) -> PathBuf {
    Path::new(ASSETS_DIR).join(name)
}

/// Frames `name` decodes to, pulled to the end.
fn decoded_frames(name: &str) -> Result<usize, AppError> {
    let decoder = FileDecoder::open(&asset(name))?;
    let channels = usize::from(decoder.channels().get());
    Ok(decoder.count() / channels)
}

/// One second at 48 kHz, which is what ffmpeg's libopus reads out of the same fixture. The priming
/// and the trailing padding both sit *inside* packets rather than beyond them, so a decoder that
/// kept either would hand back plausible audio of the wrong length.
#[test]
fn the_fixture_decodes_to_the_length_libopus_reads_out_of_it() -> Result<(), AppError> {
    assert_eq!(decoded_frames("silence.opus")?, 48_000);
    Ok(())
}

/// A header stating more priming than one packet holds. RFC 7845 section 5.1 recommends 3840
/// frames for a stream that has been cropped, and allows the count to span several packets; the
/// fixture states that over the 312 its encoder actually primed with, so the extra comes off real
/// audio and the shortfall is the assertion. Spent on the first packet alone, the rest plays.
#[test]
fn a_pre_skip_longer_than_a_packet_is_spent_across_them() -> Result<(), AppError> {
    assert_eq!(decoded_frames("silence-long-preskip.opus")?, 44_472);
    Ok(())
}

/// The demuxer's timeline counts the priming and the decoder does not hand it over, so a length
/// taken straight off it runs `pre_skip` long. lofty subtracts the same number when it reads the
/// file for the library, so without the offset the track row and the seek bar describe one file
/// with two lengths.
#[test]
fn the_length_leaves_out_the_priming_the_container_still_counts() -> Result<(), AppError> {
    let decoder = FileDecoder::open(&asset("silence.opus"))?;
    assert_eq!(decoder.total_duration(), Some(Duration::from_secs(1)));
    Ok(())
}

/// A seek is asked for on the timeline this source hands out and answered on the demuxer's, which
/// runs `pre_skip` ahead of it. Unaligned, the audio arrives that much early: half a second into a
/// one-second fixture has to leave half a second behind it, and the 312 frames either side of that
/// are the two timelines disagreeing.
#[test]
fn a_seek_lands_where_it_asked_once_the_two_timelines_agree() -> Result<(), AppError> {
    let mut decoder = FileDecoder::open(&asset("silence.opus"))?;
    decoder.try_seek(Duration::from_millis(500)).map_err(|e| AppError::Player(e.to_string()))?;

    let channels = usize::from(decoder.channels().get());
    assert_eq!(decoder.count() / channels, 24_000);
    Ok(())
}
