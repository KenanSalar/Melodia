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
use crate::player::source::audio::SampleRate;
use melodia_core::error::AppError;
use melodia_testkit::ASSETS_DIR;

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
///
/// Exactly one edit, keyed to the audio track: an entry emitted for a track that states none is
/// what would put a cover-art or chapter track's numbers on the one being decoded.
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
/// exactly the boundary they exist to test. Nothing in `test-assets/` distinguishes the two
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

/// The walk's branches that no fixture states.
///
/// ffmpeg writes one shape and it is the only one in `test-assets/`: a version 0 edit list of a
/// single entry, under 32-bit box headers, in a file with one track. Everything below is legal MP4
/// the walk has to read the same way, so the boxes are written out here the way
/// `tests/crossfade.rs` writes its WAV headers.
mod synthetic {
    use super::{asset, edit_lists};
    use melodia_core::error::AppError;
    use std::fs::File;

    /// Rate 1.0 in the 16.16 fixed point an edit list entry ends on.
    const NORMAL_RATE: u32 = 0x0001_0000;

    fn mp4_box(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len());
        out.extend_from_slice(&u32::try_from(8 + payload.len()).unwrap_or(u32::MAX).to_be_bytes());
        out.extend_from_slice(&kind);
        out.extend_from_slice(payload);
        out
    }

    /// An `mvhd`, `tkhd` or `mdhd`, carrying `field` where all three state their one.
    fn header_box(kind: [u8; 4], field: u32) -> Vec<u8> {
        header_box_of_len(kind, field, super::super::HEADER_BOX_PREFIX)
    }

    fn header_box_of_len(kind: [u8; 4], field: u32, len: usize) -> Vec<u8> {
        let mut payload = vec![0u8; len];
        if let Some(slot) = payload.get_mut(12..16) {
            slot.copy_from_slice(&field.to_be_bytes());
        }
        mp4_box(kind, &payload)
    }

    fn elst_v0(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut payload = vec![0u8; 4];
        payload.extend_from_slice(&u32::try_from(entries.len()).unwrap_or(0).to_be_bytes());
        for (segment_duration, media_time) in entries {
            payload.extend_from_slice(&segment_duration.to_be_bytes());
            payload.extend_from_slice(&media_time.to_be_bytes());
            payload.extend_from_slice(&NORMAL_RATE.to_be_bytes());
        }
        mp4_box(*b"elst", &payload)
    }

    fn elst_v1(entries: &[(u64, i64)]) -> Vec<u8> {
        let mut payload = vec![1u8, 0, 0, 0];
        payload.extend_from_slice(&u32::try_from(entries.len()).unwrap_or(0).to_be_bytes());
        for (segment_duration, media_time) in entries {
            payload.extend_from_slice(&segment_duration.to_be_bytes());
            payload.extend_from_slice(&media_time.to_be_bytes());
            payload.extend_from_slice(&NORMAL_RATE.to_be_bytes());
        }
        mp4_box(*b"elst", &payload)
    }

    /// One track, with `tkhd` written at `tkhd_len` so a short one can be stated too.
    fn trak(track_id: u32, media_timescale: u32, elst: Option<&[u8]>, tkhd_len: usize) -> Vec<u8> {
        let mut payload = header_box_of_len(*b"tkhd", track_id, tkhd_len);
        payload.extend_from_slice(&mp4_box(*b"mdia", &header_box(*b"mdhd", media_timescale)));
        if let Some(elst) = elst {
            payload.extend_from_slice(&mp4_box(*b"edts", elst));
        }
        mp4_box(*b"trak", &payload)
    }

    fn mp4(movie_timescale: u32, traks: &[Vec<u8>]) -> Vec<u8> {
        let mut moov = header_box(*b"mvhd", movie_timescale);
        for trak in traks {
            moov.extend_from_slice(trak);
        }
        let mut file = mp4_box(*b"ftyp", b"M4A \0\0\x02\0");
        file.extend_from_slice(&mp4_box(*b"moov", &moov));
        file
    }

    /// The bytes, through a real file, since the walk seeks rather than parsing a slice.
    fn walked(bytes: &[u8]) -> Result<Vec<super::super::Edit>, AppError> {
        let tmp = tempfile::TempDir::new()?;
        let path = tmp.path().join("synthetic.m4a");
        std::fs::write(&path, bytes)?;
        Ok(edit_lists(&mut File::open(&path)?))
    }

    fn single(bytes: &[u8]) -> Result<super::super::Edit, AppError> {
        let edits = walked(bytes)?;
        let [edit] = edits.as_slice() else {
            return Err(AppError::Player(format!("expected one edit, got {}", edits.len())));
        };
        Ok(super::super::Edit {
            track_id: edit.track_id,
            delay: edit.delay,
            playable: edit.playable,
        })
    }

    /// A media time of zero is a track with no priming, not an absent edit, and its segment
    /// duration is still the only statement of the presentation that excludes the trailing padding.
    #[test]
    fn an_edit_starting_at_zero_still_bounds_the_tail() -> Result<(), AppError> {
        let elst = elst_v0(&[(44_100, 0)]);
        let edit = single(&mp4(44_100, &[trak(1, 44_100, Some(&elst), 24)]))?;

        assert_eq!(edit.delay, 0);
        assert_eq!(edit.playable, Some(44_100));
        Ok(())
    }

    /// An empty edit delays the presentation and states no media time at all, so the priming is on
    /// whatever entry follows it.
    #[test]
    fn an_empty_edit_is_stepped_over_to_reach_the_one_that_states_the_priming()
    -> Result<(), AppError> {
        let elst = elst_v0(&[(500, -1), (44_100, 1024)]);
        let edit = single(&mp4(44_100, &[trak(1, 44_100, Some(&elst), 24)]))?;

        assert_eq!(edit.delay, 1024);
        assert_eq!(edit.playable, Some(44_100));
        Ok(())
    }

    /// The 64-bit entries a version 1 edit list carries, which every field is a different width in.
    #[test]
    fn a_version_1_edit_list_reads_the_same_as_a_version_0_one() -> Result<(), AppError> {
        let elst = elst_v1(&[(44_100, 1024)]);
        let edit = single(&mp4(44_100, &[trak(1, 44_100, Some(&elst), 24)]))?;

        assert_eq!(edit.delay, 1024);
        assert_eq!(edit.playable, Some(44_100));
        Ok(())
    }

    /// The case the fixtures cannot state, cover art being an `ilst` atom rather than a track: an
    /// edit list on a neighbouring track must not be handed to the one being decoded.
    #[test]
    fn an_edit_list_belonging_to_another_track_is_not_taken_for_this_ones() -> Result<(), AppError>
    {
        let elst = elst_v0(&[(44_100, 1024)]);
        let bytes = mp4(44_100, &[trak(1, 44_100, None, 24), trak(2, 44_100, Some(&elst), 24)]);

        let edit = single(&bytes)?;
        assert_eq!(edit.track_id, 2, "the edit was keyed to the track that states none");
        Ok(())
    }

    /// A header box too short to hold the field read out of it is refused rather than read past
    /// into whatever box follows, which is a track id or a timescale made of the next box's length.
    #[test]
    fn a_header_box_too_short_for_its_field_states_nothing() -> Result<(), AppError> {
        let elst = elst_v0(&[(44_100, 1024)]);
        let bytes = mp4(44_100, &[trak(1, 44_100, Some(&elst), 12)]);

        assert!(walked(&bytes)?.is_empty(), "a truncated tkhd named a track anyway");
        Ok(())
    }

    /// The fixtures are all 32-bit headers, so the escape that moves a box's length into the eight
    /// bytes after its type is walked here or nowhere.
    #[test]
    fn a_64_bit_box_header_is_walked_like_any_other() -> Result<(), AppError> {
        let elst = elst_v0(&[(44_100, 1024)]);
        let inner = trak(1, 44_100, Some(&elst), 24);

        let mut moov = header_box(*b"mvhd", 44_100);
        moov.extend_from_slice(&inner);

        // Size 1, the type, then the real 64-bit length.
        let mut wide = 1u32.to_be_bytes().to_vec();
        wide.extend_from_slice(b"moov");
        wide.extend_from_slice(&(16 + moov.len() as u64).to_be_bytes());
        wide.extend_from_slice(&moov);

        let mut bytes = mp4_box(*b"ftyp", b"M4A \0\0\x02\0");
        bytes.extend_from_slice(&wide);

        let edit = single(&bytes)?;
        assert_eq!(edit.delay, 1024);
        Ok(())
    }

    /// The fixture the rest of the suite leans on, read through this builder's own expectations, so
    /// a builder that writes boxes nothing else would accept fails here rather than silently
    /// pinning the walk against itself.
    #[test]
    fn the_builder_agrees_with_the_fixture_it_imitates() -> Result<(), AppError> {
        let real = edit_lists(&mut File::open(asset("silence.m4a"))?);
        let elst = elst_v0(&[(1_000, 1024)]);
        let built = walked(&mp4(1_000, &[trak(1, 44_100, Some(&elst), 24)]))?;

        let ([real], [built]) = (real.as_slice(), built.as_slice()) else {
            return Err(AppError::Player("expected one edit from each".to_owned()));
        };
        assert_eq!(
            (built.track_id, built.delay, built.playable),
            (real.track_id, real.delay, real.playable)
        );
        Ok(())
    }
}
