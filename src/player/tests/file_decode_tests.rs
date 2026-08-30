//! Tests for the file decoder.
//!
//! Every fixture named here is a container the 0.6 feature list has to carry. They read as
//! duration tests and they are also the pin on that list: drop `mkv`, `caf`, `aiff`, `pcm` or
//! `adpcm` from the manifest and the matching case goes red rather than the format quietly
//! scanning, listing, and then refusing to play.

use std::path::{Path, PathBuf};

use rodio::Source;

use super::{FileDecoder, probe_duration};
use crate::error::AppError;
use crate::test_support::ASSETS_DIR;

fn asset(name: &str) -> PathBuf {
    Path::new(ASSETS_DIR).join(name)
}

/// The function exists for the containers lofty can't identify, so those are the cases worth
/// pinning: without this answer their rows reach the library reading 0:00.
#[test]
fn probe_duration_reads_the_containers_lofty_cannot() {
    for fixture in ["silence.mka", "silence.caf"] {
        let probed = probe_duration(&asset(fixture));
        assert_eq!(probed.map(|d| d.as_secs()), Some(1), "{fixture}");
    }
}

/// Two files that demux and then need a codec registered separately from their container:
/// AIFF-C's A-law, which the common chunk resolves and `symphonia-pcm` decodes, and the MS ADPCM
/// `symphonia-adpcm` exists for. Losing either leaves a file that scans, lists, and then refuses
/// to play, which nothing reading tags would notice.
#[test]
fn probe_duration_decodes_past_the_container_to_the_codec() {
    for fixture in ["silence.aifc", "silence-adpcm.wav"] {
        let probed = probe_duration(&asset(fixture));
        assert_eq!(probed.map(|d| d.as_secs()), Some(1), "{fixture}");
    }
}

#[test]
fn probe_duration_is_none_when_nothing_decodes() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;
    let junk = tmp.path().join("not-audio.mka");
    std::fs::write(&junk, b"not valid audio")?;
    assert_eq!(probe_duration(&junk), None);
    Ok(())
}

/// Every extension `media::AUDIO_EXTENSIONS` offers has to reach a decoder, and a fixture that
/// opens is the only thing that says so.
#[test]
fn every_fixture_container_opens() -> Result<(), AppError> {
    for fixture in [
        "silence.mp3",
        "silence.flac",
        "silence.m4a",
        "silence.ogg",
        "silence.wav",
        "silence.aiff",
        "silence.aifc",
        "silence.mka",
        "silence.caf",
        "silence-adpcm.wav",
    ] {
        let decoder = FileDecoder::open(&asset(fixture))?;
        assert!(decoder.sample_rate().get() > 0, "{fixture}");
        assert!(decoder.channels().get() > 0, "{fixture}");
    }
    Ok(())
}

/// A span of zero would have the mixer rebuild its resampler against an empty `Take`, and `None`
/// would pin it to whatever reached the deck first — the fault `tests/stream_rate.rs` covers for
/// the ring, on the path that feeds the same mixer.
#[test]
fn the_span_names_a_real_packet() -> Result<(), AppError> {
    let decoder = FileDecoder::open(&asset("silence.flac"))?;
    let span = decoder.current_span_len();
    assert!(span.is_some_and(|len| len > 0), "{span:?}");
    Ok(())
}

/// A seek lands on a packet boundary, so the trim is what makes the position asked for the
/// position played. Without it the head of the packet replays, which is up to a packet of audio
/// the listener already heard.
#[test]
fn a_seek_lands_on_the_frame_it_asked_for() -> Result<(), AppError> {
    const SEEK_MS: u64 = 500;

    let mut decoder = FileDecoder::open(&asset("silence.wav"))?;
    let rate = u64::from(decoder.sample_rate().get());
    let channels = u64::from(decoder.channels().get());

    // Pull one sample so the first packet is the one being stepped past, then seek into the
    // middle of a packet rather than onto its edge.
    assert!(decoder.next().is_some());
    let seek = std::time::Duration::from_millis(SEEK_MS);
    decoder.try_seek(seek).map_err(|e| AppError::Player(e.to_string()))?;

    // What remains has to be the tail of the file from the target onwards, to within the frame
    // the trim rounds to.
    let remaining = u64::try_from(decoder.count()).unwrap_or(u64::MAX) / channels;
    let expected = rate - rate * SEEK_MS / 1000;
    let drift = remaining.abs_diff(expected);
    assert!(drift <= 1, "{remaining} frames left, expected about {expected}");
    Ok(())
}
