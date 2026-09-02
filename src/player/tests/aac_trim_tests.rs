//! Tests for the AAC encoder padding an MP4 states.
//!
//! The two halves are pinned separately because they fail separately: the string parse against
//! values real encoders write, and the box walk against the fixtures, whose `elst` came from
//! ffmpeg and states a priming nothing here chose.

use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::meta::MetadataLog;
use symphonia::core::units::TimeBase;

use super::{edit_lists, exact_media_ticks, parse_smpb};
use crate::error::AppError;
use crate::player::audio::SampleRate;
use crate::test_support::ASSETS_DIR;

/// What iTunes writes: priming 0x840 (2112, Apple's default), remainder 0x1DC, and a count of
/// 0xAC44E4 samples. Twelve fields, of which three carry anything.
const ITUNES: &str = " 00000000 00000840 000001DC 0000000000AC44E4 00000000 00000000 \
                      00000000 00000000 00000000 00000000 00000000 00000000";

fn asset(name: &str) -> PathBuf {
    Path::new(ASSETS_DIR).join(name)
}

#[test]
fn itunsmpb_yields_the_priming_and_the_original_sample_count() -> Result<(), AppError> {
    let Some(smpb) = parse_smpb(ITUNES) else {
        return Err(AppError::Player("a well-formed iTunSMPB did not parse".to_owned()));
    };
    assert_eq!(smpb.priming, 2112);
    assert_eq!(smpb.frames, 0x00AC_44E4);
    Ok(())
}

/// The remainder sits between the two fields that are read, so a parser taking them in order
/// rather than by position reads the padding as the length.
#[test]
fn the_remainder_field_is_stepped_over_rather_than_read() -> Result<(), AppError> {
    let Some(smpb) = parse_smpb(" 00000000 00000840 000001DC 0000000000009C40") else {
        return Err(AppError::Player("a four-field iTunSMPB did not parse".to_owned()));
    };
    assert_eq!(smpb.frames, 40000, "the count was read from the remainder's field");
    Ok(())
}

#[test]
fn an_itunsmpb_that_says_nothing_is_refused() {
    let refused = [
        ("too few fields", " 00000000 00000840 000001DC"),
        ("a zero sample count", " 00000000 00000840 000001DC 0000000000000000"),
        ("not hexadecimal", " 00000000 nonsense 000001DC 0000000000009C40"),
        ("empty", ""),
    ];

    for (what, value) in refused {
        assert!(parse_smpb(value).is_none(), "{what}");
    }
}

/// ffmpeg writes the movie header in milliseconds against 44100 media ticks, and at that ratio a
/// converted segment duration is worth less than the padding it would remove. Dropping it is what
/// keeps the derived length, which for such a file is exact, from being overruled by a rounding.
#[test]
fn a_segment_duration_is_only_converted_where_it_loses_nothing() {
    assert_eq!(exact_media_ticks(220_502, 44_100, 44_100), Some(220_502));
    assert_eq!(exact_media_ticks(1_000, 1_000, 44_100), Some(44_100));
    assert_eq!(exact_media_ticks(14_814, 1_000, 44_100), None);
    assert_eq!(exact_media_ticks(1_000, 0, 44_100), None);
}

/// The number the fixtures were generated with, and the reason the end-to-end counts in
/// `file_decode_tests` are what they are.
#[test]
fn the_walk_reads_the_edit_list_the_fixtures_carry() -> Result<(), AppError> {
    for fixture in ["silence.m4a", "silence.m4b", "silence-cover.m4a"] {
        let mut file = File::open(asset(fixture))?;
        let edits = edit_lists(&mut file);

        let [edit] = edits.as_slice() else {
            return Err(AppError::Player(format!("{fixture}: expected one edit list")));
        };
        assert_eq!(edit.track_id, 1, "{fixture}");
        assert_eq!(edit.delay, 1024, "{fixture}");
        // A whole second stated in milliseconds, so it converts exactly even at the coarse movie
        // timescale, and agrees with the derived length the resolution would otherwise fall back on.
        assert_eq!(edit.playable, Some(44_100), "{fixture}");
    }
    Ok(())
}

/// A cover-art track sits beside the audio one, and only the audio track's own numbers may reach
/// the decoder. Keyed rather than positional, so a file listing them the other way round is the
/// same answer.
#[test]
fn an_edit_list_is_keyed_to_the_track_that_states_it() -> Result<(), AppError> {
    let mut file = File::open(asset("silence-cover.m4a"))?;
    let edits = edit_lists(&mut file);

    assert!(
        edits.iter().all(|edit| edit.track_id == 1),
        "a track with no edit list of its own must contribute none"
    );
    Ok(())
}

/// The walk runs on every file opened, not only the MP4s, so what it does with the others is as
/// load-bearing as what it reads out of these.
#[test]
fn nothing_is_read_out_of_a_file_that_is_not_an_mp4() -> Result<(), AppError> {
    for fixture in [
        "silence.mp3",
        "silence.flac",
        "silence.aac",
        "silence.ogg",
        "silence.wav",
    ] {
        let mut file = File::open(asset(fixture))?;
        assert!(edit_lists(&mut file).is_empty(), "{fixture}");
    }
    Ok(())
}

/// The demuxer reads the same handle straight afterwards, and a probe starting partway into the
/// file resolves no format at all.
#[test]
fn the_walk_leaves_the_handle_where_it_found_it() -> Result<(), AppError> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = File::open(asset("silence.m4a"))?;
    file.seek(SeekFrom::Start(0))?;
    let _ = edit_lists(&mut file);

    let mut opening = [0u8; 8];
    file.read_exact(&mut opening)?;
    assert_eq!(&opening[4..], b"ftyp", "the demuxer would open midway through the file");
    Ok(())
}

/// A malformed file reaches here ahead of the demuxer that would reject it, so the walk answers
/// rather than looping or reading past the end.
#[test]
fn a_malformed_box_tree_is_given_up_on() -> Result<(), AppError> {
    let tmp = tempfile::TempDir::new()?;

    let cases: [(&str, &[u8]); 4] = [
        // A box claiming to be larger than the file.
        ("oversized", b"\x7f\xff\xff\xffmoov"),
        // A box whose stated size cannot advance the walk.
        ("zero-width header", b"\x00\x00\x00\x02moov\x00\x00\x00\x02trak"),
        // A truncated header.
        ("truncated", b"\x00\x00"),
        ("empty", b""),
    ];

    for (what, bytes) in cases {
        let path = tmp.path().join(format!("{what}.m4a"));
        std::fs::write(&path, bytes)?;
        let mut file = File::open(&path)?;
        assert!(edit_lists(&mut file).is_empty(), "{what}");
    }
    Ok(())
}

/// The bug the Fraunhofer gapless pair caught, pinned without committing it.
///
/// Those files state 220502 frames of presentation over a 222208-frame track carrying 1600 of
/// priming, so deriving the length as duration minus priming leaves 106 frames of padding at
/// exactly the boundary they exist to test. Nothing in `tests/assets/` distinguishes the two
/// answers, every fixture there agreeing on both, so this states them directly.
#[test]
fn the_edit_lists_own_length_wins_over_the_one_derived_from_the_duration() -> Result<(), AppError> {
    let Some(trim) = resolved(Some(222_208), 1600, Some(220_502)) else {
        return Err(AppError::Player("an edit list stating both resolved to nothing".to_owned()));
    };
    assert_eq!(trim.head, 1600);
    assert_eq!(trim.playable, Some(220_502), "the derived length overruled the stated one");
    Ok(())
}

/// The other direction, because the stated length is the container's claim rather than a fact: an
/// edit presenting more audio than the track holds would run the source past its own end.
#[test]
fn an_edit_list_may_not_present_more_than_the_track_holds() -> Result<(), AppError> {
    let Some(trim) = resolved(Some(222_208), 1600, Some(999_999)) else {
        return Err(AppError::Player("an overlong edit list resolved to nothing".to_owned()));
    };
    assert_eq!(trim.playable, Some(220_608), "an overlong edit was taken at its word");
    Ok(())
}

/// Where the edit list states no usable length, the track duration is what is left to ask.
#[test]
fn the_duration_answers_where_the_edit_lists_own_length_cannot() -> Result<(), AppError> {
    let Some(trim) = resolved(Some(45_124), 1024, None) else {
        return Err(AppError::Player("an edit list with no length resolved to nothing".to_owned()));
    };
    assert_eq!(trim.playable, Some(44_100));
    Ok(())
}

/// A priming this large is a misread or an edit list being used for something else, and acting on
/// it would cut a second of real audio off the front.
#[test]
fn an_implausible_priming_is_refused_rather_than_acted_on() {
    assert!(resolved(Some(4_410_000), 44_101, None).is_none());
}

/// [`resolve`] against a track stated field by field, since no fixture states these combinations.
///
/// The timebase is a plain 1/44100, which is what an MP4 audio track carries, and the metadata log
/// is empty so the edit list is what answers rather than an `iTunSMPB`.
fn resolved(duration: Option<u64>, delay: u64, playable: Option<u64>) -> Option<super::Trim> {
    let rate = SampleRate::new(44_100)?;
    let timing = super::Timing {
        id: 1,
        time_base: TimeBase::try_new(1, 44_100)?,
        duration,
    };
    let edits = [super::Edit {
        track_id: 1,
        delay,
        playable,
    }];
    let mut log = MetadataLog::default();
    super::resolve(&timing, &log.metadata(), &edits, rate)
}
