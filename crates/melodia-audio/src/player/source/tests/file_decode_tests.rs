//! Tests for the file decoder.
//!
//! The fixtures pin the 0.6 feature list as much as the decoder: drop `aac`, `mkv`, `caf`, `aiff`,
//! `pcm` or `adpcm` from the manifest and the matching case goes red rather than the format quietly
//! scanning, listing, and then refusing to play.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{FileDecoder, probe_duration};
use crate::player::source::aac_trim;
use crate::player::source::audio::AudioSource;
use crate::test_support::ASSETS_DIR;
use melodia_core::error::AppError;

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
    for extension in melodia_core::utils::audio_ext::AUDIO_EXTENSIONS {
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

/// The frames a source hands over, and the length it reports for them.
fn decoded(fixture: &Path) -> Result<(u64, Option<Duration>), AppError> {
    let decoder = FileDecoder::open(fixture)?;
    let channels = u64::from(decoder.channels().get());
    let total = decoder.total_duration();
    let frames = u64::try_from(decoder.count()).unwrap_or(u64::MAX) / channels;
    Ok((frames, total))
}

/// The point of the feature, counted rather than listened to.
///
/// The fixtures are a second of silence each and their edit lists state 1024 frames of priming
/// over 45 packets of 1024, so an untrimmed decode hands over 46080 frames and reads 1.023 s long.
/// A container's own answer is 44100, which is also what lofty reports for the same file, so the
/// number to check is the one the rest of the app already believes.
#[test]
fn an_aac_file_hands_over_what_its_container_says_it_holds() -> Result<(), AppError> {
    for fixture in ["silence.m4a", "silence.m4b", "silence-cover.m4a"] {
        let (frames, total) = decoded(&asset(fixture))?;
        assert_eq!(frames, 44_100, "{fixture}");
        assert_eq!(total, Some(Duration::from_secs(1)), "{fixture}");
    }
    Ok(())
}

/// Raw ADTS has nowhere to state a priming, so the one AAC case with no container answer must come
/// through untouched rather than be trimmed by whatever a neighbouring file said.
#[test]
fn a_raw_adts_stream_is_left_exactly_as_it_decodes() -> Result<(), AppError> {
    let (frames, _) = decoded(&asset("silence.aac"))?;
    assert_eq!(frames, 46_080);
    Ok(())
}

/// The box walk runs on every file opened, so a format it cannot read must decode to what it
/// always did. These two carry their own gapless answers — LAME headers, and nothing to trim —
/// and neither is this module's to touch.
#[test]
fn a_file_that_is_not_aac_decodes_to_what_it_always_did() -> Result<(), AppError> {
    for fixture in ["silence.mp3", "silence.flac", "silence.wav", "silence.ogg"] {
        let (frames, _) = decoded(&asset(fixture))?;
        assert_eq!(frames, 44_100, "{fixture}");
    }
    Ok(())
}

/// The half no fixture states, built here rather than committed: no encoder to hand writes
/// `iTunSMPB`, and the tag is the one iTunes, qaac and Apple Music put it in.
///
/// The numbers are deliberately not the edit list's, so this pins the precedence as well as the
/// parse: the same file states 1024 of priming over 44100 playable underneath, and the tag has to
/// win both.
#[test]
fn itunsmpb_is_read_off_the_file_and_beats_the_edit_list_under_it() -> Result<(), AppError> {
    // Priming 0x840 (2112), a remainder, and 0x9C40 (40000) samples of audio.
    const SMPB: &str = " 00000000 00000840 00000100 0000000000009C40 00000000 00000000 \
                        00000000 00000000 00000000 00000000 00000000 00000000";

    let tmp = tempfile::TempDir::new()?;
    let tagged = tmp.path().join("smpb.m4a");
    write_smpb(&tagged, SMPB)?;

    // The tag write rewrites `moov`, and a copy that came back without its edit list would leave
    // nothing here for the tag to have precedence over.
    let edits = aac_trim::edit_lists(&mut File::open(&tagged)?);
    let [edit] = edits.as_slice() else {
        return Err(AppError::Player("the tag write dropped the edit list".to_owned()));
    };
    assert_eq!(edit.delay, 1024);

    let (frames, _) = decoded(&tagged)?;
    assert_eq!(frames, 40_000, "the tag's own sample count did not win");
    Ok(())
}

/// The count alone cannot see the priming: a tail cap of 44100 frames yields 44100 either way, and
/// the fixtures are silence, so nothing in their content says where the audio starts. An
/// `iTunSMPB` overstating the length removes the cap, leaving the source to run to its own end,
/// and what it hands over is then exactly the packets minus the priming.
///
/// It pins the overstatement too, which is a container's claim rather than a fact: 45 packets hold
/// 46080 frames however many the tag asks for.
#[test]
fn the_priming_is_dropped_even_when_nothing_bounds_the_tail() -> Result<(), AppError> {
    // Priming 0x840 (2112), against a count far past what the file holds.
    const OVERSTATED: &str = " 00000000 00000840 00000000 00000000000F4240";

    let tmp = tempfile::TempDir::new()?;
    let tagged = tmp.path().join("overstated.m4a");
    write_smpb(&tagged, OVERSTATED)?;

    let (frames, _) = decoded(&tagged)?;
    assert_eq!(frames, 46_080 - 2112, "the priming was not dropped");
    Ok(())
}

/// The tail is recounted against where the seek landed rather than carried over from the open, so
/// what is left is half of the *playable* second and not half of the 1.023 s the packets hold.
#[test]
fn a_seek_recounts_the_playable_tail_from_where_it_landed() -> Result<(), AppError> {
    const SEEK_MS: u64 = 500;

    let mut decoder = FileDecoder::open(&asset("silence.m4a"))?;
    let rate = u64::from(decoder.sample_rate().get());
    let frames = frames_left_after_seeking(&mut decoder, SEEK_MS)?;

    let expected = rate - rate * SEEK_MS / 1000;
    assert!(
        frames.abs_diff(expected) <= 1,
        "{frames} frames left after seeking to {SEEK_MS}ms, expected about {expected}"
    );
    Ok(())
}

/// A seek is expressed on the trimmed timeline while the demuxer still counts from the priming, so
/// the head has to be added back on the way in. Without it every seek into a trimmed file lands
/// early by the priming, which is inaudible on one track and cumulative across a queue.
///
/// Read through an overstated `iTunSMPB` because the test above cannot see this: its tail cap is a
/// function of the position asked for, so it holds the count at the same number wherever the
/// demuxer actually put the reader. With the cap past the end of the file, what is left to decode
/// is the answer.
#[test]
fn a_seek_into_a_trimmed_file_steps_over_the_priming() -> Result<(), AppError> {
    // Priming 0x840 (2112), against a count far past what the file holds.
    const OVERSTATED: &str = " 00000000 00000840 00000000 00000000000F4240";
    const SEEK_MS: u64 = 500;

    let tmp = tempfile::TempDir::new()?;
    let tagged = tmp.path().join("seek.m4a");
    write_smpb(&tagged, OVERSTATED)?;

    let mut decoder = FileDecoder::open(&tagged)?;
    let rate = u64::from(decoder.sample_rate().get());
    let frames = frames_left_after_seeking(&mut decoder, SEEK_MS)?;

    // The 45 packets the file holds, less half a second and the priming ahead of it.
    let expected = 46_080 - (rate * SEEK_MS / 1000 + 2112);
    assert!(
        frames.abs_diff(expected) <= 1,
        "{frames} frames left after seeking to {SEEK_MS}ms, expected about {expected}"
    );
    Ok(())
}

/// Frames the source still hands over once it has been seeked to `seek_ms`.
fn frames_left_after_seeking(decoder: &mut FileDecoder, seek_ms: u64) -> Result<u64, AppError> {
    let channels = u64::from(decoder.channels().get());
    decoder
        .try_seek(Duration::from_millis(seek_ms))
        .map_err(|e| AppError::Player(e.to_string()))?;
    Ok(u64::try_from(decoder.by_ref().count()).unwrap_or(u64::MAX) / channels)
}

/// A copy of the AAC fixture carrying `value` as its `iTunSMPB`.
///
/// Written rather than committed because no encoder to hand produces one: ffmpeg writes an edit
/// list instead, and the tag is what iTunes, qaac and Apple Music put the same numbers in.
fn write_smpb(path: &Path, value: &str) -> Result<(), AppError> {
    use lofty::config::WriteOptions;
    use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};
    use lofty::prelude::TagExt;

    std::fs::copy(asset("silence.m4a"), path)?;

    let mut ilst = Ilst::new();
    ilst.insert(Atom::new(
        AtomIdent::Freeform {
            mean: "com.apple.iTunes".into(),
            name: "iTunSMPB".into(),
        },
        AtomData::UTF8(value.to_owned()),
    ));
    ilst.save_to_path(path, WriteOptions::default())
        .map_err(|e| AppError::Player(format!("could not write iTunSMPB: {e}")))
}
