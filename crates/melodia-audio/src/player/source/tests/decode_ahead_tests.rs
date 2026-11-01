//! Tests for a file decoded ahead of its deck: that the ring hands out exactly what the decoder
//! does, on both sides of the depth, and that a file which has ended doesn't hold its deck.

use std::time::{Duration, Instant};

use super::{DEPTH, DecodeAhead};
use crate::player::source::audio::{AudioSource, Sample};
use crate::player::source::file_decode::FileDecoder;
use crate::player::source::tests::helpers::asset;
use melodia_core::error::AppError;

/// One second of stereo with a different constant per channel, so a sample lost or doubled
/// anywhere flips every channel after it.
const FIXTURE: &str = "stereo-dc.wav";

/// Well short of the fixture, so the first fill stops at a full ring and a feeder decodes the rest.
const SHALLOW: Duration = Duration::from_millis(250);

/// How long a drain waits on the feeder before handing back what it has, so a feeder that never
/// finishes fails the comparison rather than hanging the suite.
const PATIENCE: Duration = Duration::from_secs(10);

fn decoded_directly() -> Result<Vec<Sample>, AppError> {
    Ok(FileDecoder::open(&asset(FIXTURE))?.collect())
}

fn decoded_ahead(depth: Duration) -> Result<DecodeAhead, AppError> {
    DecodeAhead::start(FileDecoder::open(&asset(FIXTURE))?, depth)
}

/// Pulls as a deck does, only while the source is ready, until it ends, `most` samples are in hand
/// or [`PATIENCE`] runs out.
fn drain_as_a_deck(mut source: DecodeAhead, most: usize) -> Vec<Sample> {
    let deadline = Instant::now() + PATIENCE;
    let mut pulled = Vec::new();
    while pulled.len() < most && Instant::now() < deadline {
        if !source.is_ready() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let Some(sample) = source.next() else { break };
        pulled.push(sample);
    }
    pulled
}

fn bits(samples: &[Sample]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

/// Shorter than the depth, the whole file decodes before `start` returns and the source ends on
/// its own. One sample past the decoder's count is asked for, which a source never told the file
/// ended would answer with starvation silence.
#[test]
fn a_file_inside_the_depth_hands_out_exactly_what_its_decoder_does() -> Result<(), AppError> {
    let expected = decoded_directly()?;
    let source = decoded_ahead(DEPTH)?;

    let pulled: Vec<Sample> = source.take(expected.len() + 1).collect();

    assert_eq!(pulled.len(), expected.len(), "samples handed out");
    assert_eq!(bits(&pulled), bits(&expected));
    Ok(())
}

/// Past the depth, the caller's fill hands the decoder to a feeder part way through, and nothing
/// may be dropped or decoded twice at that seam.
#[test]
fn a_file_past_the_depth_hands_out_exactly_what_its_decoder_does() -> Result<(), AppError> {
    let expected = decoded_directly()?;
    let source = decoded_ahead(SHALLOW)?;

    let pulled = drain_as_a_deck(source, expected.len() + 1);

    assert_eq!(pulled.len(), expected.len(), "samples handed out");
    assert_eq!(bits(&pulled), bits(&expected));
    Ok(())
}

/// The last stretch of every file sits below the low-water mark with nothing left to arrive. A
/// source holding there would never reach its end, and the track would never advance.
#[test]
fn a_file_that_has_ended_stays_ready_to_its_last_frame() -> Result<(), AppError> {
    let samples = decoded_directly()?.len();
    let mut source = decoded_ahead(DEPTH)?;
    source.by_ref().take(samples - 2).for_each(drop);

    let ready = source.is_ready();

    assert!(ready, "the file's last frame was held back");
    Ok(())
}
