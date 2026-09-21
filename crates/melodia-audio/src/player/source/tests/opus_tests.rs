//! Tests for the Opus decoder.
//!
//! Both cases are frame counts, which is the only assertion that can see a pre-skip or an end-trim
//! regression: the samples either side of one are still the right samples, so a comparison against
//! a reference decode would agree on everything it looked at and disagree only on how much there
//! was.

use std::path::{Path, PathBuf};

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
