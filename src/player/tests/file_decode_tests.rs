//! Tests for the file decoder.
//!
//! The fixtures pin the 0.6 feature list as much as the decoder: drop `aac`, `mkv`, `caf`, `aiff`,
//! `pcm` or `adpcm` from the manifest and the matching case goes red rather than the format quietly
//! scanning, listing, and then refusing to play.

use std::path::{Path, PathBuf};

use super::{FileDecoder, probe_duration};
use crate::error::AppError;
use crate::player::audio::AudioSource;
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

/// Every extension the scanner offers has to reach a decoder, and a fixture that opens is the only
/// thing that says so.
///
/// Walked rather than listed, because a list cannot see the entry that is missing from it: an
/// extension added to `AUDIO_EXTENSIONS` with no fixture beside it fails here, rather than shipping
/// as a format the library offers and nothing plays. `aac` is why — raw ADTS is the case the whole
/// move to 0.6 was for, and it went untested under a hand-written list.
#[test]
fn every_scanned_extension_reaches_a_decoder() -> Result<(), AppError> {
    for extension in crate::media::AUDIO_EXTENSIONS {
        let decoder = FileDecoder::open(&asset(&format!("silence.{extension}")))?;
        assert!(decoder.sample_rate().get() > 0, "{extension}");
        assert!(decoder.channels().get() > 0, "{extension}");
    }
    Ok(())
}

/// `AudioSource::try_seek` saturates wherever a length is known, and the caller asks past the end
/// routinely: the position it seeks to comes off the tags, which overshoot the decoded length by a
/// few frames often enough. Unclamped the demuxer answers out of range or parks at the end, and a
/// deck draining reads to the monitor as the track finishing — the queue jumps, from a drag of the
/// slider to its own right edge.
///
/// Two assertions because the formats fail differently: some answer out of range, and the rest
/// return `Ok` having landed with nothing left to play. A WAV is the second kind, so the seek
/// succeeding is exactly what a length check alone would have believed.
#[test]
fn a_seek_past_the_end_saturates_rather_than_failing() -> Result<(), AppError> {
    for fixture in [
        "silence.wav",
        "silence.mp3",
        "silence.m4a",
        "silence.ogg",
        "silence.flac",
    ] {
        let mut decoder = FileDecoder::open(&asset(fixture))?;
        let Some(length) = decoder.total_duration() else {
            return Err(AppError::Player(format!("{fixture} states no length to clamp to")));
        };

        let seeked = decoder.try_seek(length * 4);
        assert!(seeked.is_ok(), "{fixture}: a seek past the end must saturate: {seeked:?}");
        assert!(decoder.next().is_some(), "{fixture}: and must leave audio behind it");
    }
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

/// The seek puts the puller back on the channel it was part way through, and nothing downstream
/// re-syncs, so getting this wrong swaps the stereo image for the rest of the track.
///
/// Driven by hand because the deck's converter cannot be the caller that reaches it — it takes
/// whole frames and seeks between them. `stereo-dc.wav` carries a different constant per channel,
/// which is the only way to tell which one a sample came from; every other fixture is mono, where
/// the fault is invisible.
#[test]
fn a_seek_resumes_on_the_channel_it_was_part_way_through() -> Result<(), AppError> {
    const LEFT: f32 = 0.5;
    const RIGHT: f32 = 0.25;

    let mut decoder = FileDecoder::open(&asset("stereo-dc.wav"))?;

    // One sample leaves the puller owed channel 1.
    let Some(first) = decoder.next() else {
        return Err(AppError::Player("the fixture decoded nothing".to_owned()));
    };
    assert!((first - LEFT).abs() < 1e-3, "the fixture opened on {first}, not its left channel");
    decoder
        .try_seek(std::time::Duration::from_millis(500))
        .map_err(|e| AppError::Player(e.to_string()))?;

    let Some(resumed) = decoder.next() else {
        return Err(AppError::Player("the seek left nothing to play".to_owned()));
    };
    assert!(
        (resumed - RIGHT).abs() < 1e-3,
        "resumed on {resumed}, which is the left channel where the right was due"
    );
    Ok(())
}
