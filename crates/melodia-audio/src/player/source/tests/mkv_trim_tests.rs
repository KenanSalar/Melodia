//! Tests for the trailing padding a Matroska file states.
//!
//! The three halves fail separately: the walk against the one fixture carrying the element, the
//! resolution against bounds no fixture states, and the EBML primitives against containers written
//! here, ffmpeg producing exactly one shape where the spec admits many.

use std::fs::File;

use super::{Discard, discard_padding, resolve};
use crate::player::source::tests::helpers::{asset, nz_u32};
use melodia_core::error::AppError;

/// What `silence-opus.mka` states, and the 648 frames `opus_tests` counts off the other end of it.
const FIXTURE_TICKS: u64 = 13_500_000;
const FIXTURE_FRAMES: u64 = 648;

const RATE: u32 = 48_000;

/// Matroska ticks in a second. Not scaled by `TimestampScale`, and not divisible by [`RATE`], which
/// is why every bound below sits either side of a fraction rather than on a round tick.
const TICKS_PER_SECOND: u64 = 1_000_000_000;

fn only_discard(discards: &[Discard]) -> Result<(u32, u64), AppError> {
    let [discard] = discards else {
        return Err(AppError::Player(format!("expected one discard, got {}", discards.len())));
    };
    Ok((discard.track_num, discard.ticks))
}

#[test]
fn the_walk_reads_the_padding_the_fixture_states() -> Result<(), AppError> {
    let mut file = File::open(asset("silence-opus.mka"))?;

    let stated = only_discard(&discard_padding(&mut file))?;

    assert_eq!(stated, (1, FIXTURE_TICKS));
    Ok(())
}

/// The element is optional, and a file without one is the common case: nothing but a muxer that
/// knows the encoder's tail writes it.
#[test]
fn a_matroska_file_that_states_no_padding_answers_none() -> Result<(), AppError> {
    let mut file = File::open(asset("silence.mka"))?;

    assert!(discard_padding(&mut file).is_empty());
    Ok(())
}

/// The walk runs on every file opened, not only the Matroska ones, so what it does with the others
/// is as load-bearing as what it reads out of these.
#[test]
fn nothing_is_read_out_of_a_file_that_is_not_matroska() -> Result<(), AppError> {
    for fixture in ["silence.mp3", "silence.flac", "silence.m4a", "silence.ogg", "silence.wav"] {
        let mut file = File::open(asset(fixture))?;
        assert!(discard_padding(&mut file).is_empty(), "{fixture}");
    }
    Ok(())
}

/// The demuxer reads the same handle straight afterwards, and a probe starting partway into the
/// file resolves no format at all.
#[test]
fn the_walk_leaves_the_handle_where_it_found_it() -> Result<(), AppError> {
    use std::io::Read;

    let mut file = File::open(asset("silence-opus.mka"))?;
    let _ = discard_padding(&mut file);

    let mut opening = [0u8; 4];
    file.read_exact(&mut opening)?;
    assert_eq!(opening, [0x1A, 0x45, 0xDF, 0xA3], "the demuxer would open midway through the file");
    Ok(())
}

/// An `.mka` may hold more than one track, and a padding stated for a second audio or a subtitle
/// track means nothing to the one being decoded.
#[test]
fn a_padding_stated_for_another_track_is_not_taken_for_this_ones() {
    let discards = [Discard { track_num: 2, ticks: FIXTURE_TICKS }];
    let rate = nz_u32(RATE);

    assert_eq!(resolve(&discards, 2, rate), Some(FIXTURE_FRAMES));
    assert_eq!(resolve(&discards, 1, rate), None);
}

/// No encoder pads anything near a second, so a larger number is a misread or a muxer using the
/// element for something else, and acting on it would cut real audio off the end.
///
/// Either side of the ceiling rather than on it alone: a bound written `<` where it means `<=`
/// refuses the longest padding a file may legitimately state.
#[test]
fn a_tail_longer_than_the_ceiling_is_refused_rather_than_acted_on() {
    let rate = nz_u32(RATE);
    let resolved = |ticks| resolve(&[Discard { track_num: 1, ticks }], 1, rate);

    assert_eq!(resolved(TICKS_PER_SECOND), Some(u64::from(RATE)));
    // The first tick count resolving to one frame past a second's worth.
    assert_eq!(resolved(1_000_020_834), None);
}

/// Rounded down, so a tail that overshoots cannot take real audio with it. That leaves a padding
/// shorter than one frame resolving to nothing, and a tail of no frames is no tail.
#[test]
fn a_tail_that_rounds_to_nothing_is_no_tail_at_all() {
    let rate = nz_u32(RATE);
    let resolved = |ticks| resolve(&[Discard { track_num: 1, ticks }], 1, rate);

    // A frame is 20833.3 ticks at this rate, so these sit either side of one.
    assert_eq!(resolved(20_833), None);
    assert_eq!(resolved(20_834), Some(1));
    assert_eq!(resolved(0), None);
}

/// The branches the fixture cannot state.
///
/// ffmpeg writes one shape and it is the only one in `test-assets/`: a single sized cluster holding
/// one block group, under a sized segment, in a file with one track. Everything below is legal
/// Matroska the walk has to read the same way, so the elements are written out here the way
/// `aac_trim_tests::synthetic` writes its MP4 boxes.
mod synthetic {
    use std::fs::File;

    use super::super::{Discard, MAX_ELEMENT_HEADERS, discard_padding};
    use super::{FIXTURE_TICKS, only_discard};
    use crate::player::source::tests::helpers::asset;
    use melodia_core::error::AppError;

    const EBML_HEADER: u32 = 0x1A45_DFA3;
    const SEGMENT: u32 = 0x1853_8067;
    const CLUSTER: u32 = 0x1F43_B675;
    const BLOCK_GROUP: u32 = 0xA0;
    const BLOCK: u32 = 0xA1;
    const DISCARD_PADDING: u32 = 0x75A2;
    /// `Void`, so the walk has an element to step over that it does not name.
    const VOID: u32 = 0xEC;

    /// [`FIXTURE_TICKS`] as the signed element spells it, kept derived so the two cannot drift.
    fn fixture_padding() -> i64 {
        i64::try_from(FIXTURE_TICKS).unwrap_or(i64::MAX)
    }

    /// An id as a file spells it, marker bits kept: the leading zero bytes of the constants above
    /// are not part of the encoding.
    fn id_bytes(id: u32) -> Vec<u8> {
        let bytes = id.to_be_bytes();
        let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len() - 1);
        bytes.get(first..).unwrap_or_default().to_vec()
    }

    /// A size as an eight-byte vint, the widest EBML admits and so the one whose first byte carries
    /// no payload bits at all.
    fn size_vint(value: u64) -> [u8; 8] {
        let mut bytes = value.to_be_bytes();
        if let Some(marker) = bytes.first_mut() {
            *marker = 0x01;
        }
        bytes
    }

    /// The width-`width` vint stating an unknown size: every payload bit set.
    fn unknown_size_vint(width: usize) -> Vec<u8> {
        let mut bytes = vec![0xFFu8; width];
        if let Some(marker) = bytes.first_mut() {
            *marker = 0xFFu8 >> (width - 1);
        }
        bytes
    }

    fn element(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.extend_from_slice(&size_vint(u64::try_from(payload.len()).unwrap_or(u64::MAX)));
        out.extend_from_slice(payload);
        out
    }

    /// An element stating no size of its own, which runs to the end of its parent.
    fn open_ended(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.extend_from_slice(&unknown_size_vint(8));
        out.extend_from_slice(payload);
        out
    }

    /// A block, whose first field is a track number in a vint of the same shape as a size.
    fn block(track: u32) -> Vec<u8> {
        let mut payload = size_vint(u64::from(track)).to_vec();
        // A timestamp and flags, which the walk steps over without reading.
        payload.extend_from_slice(&[0x00, 0x00, 0x00]);
        element(BLOCK, &payload)
    }

    fn block_group(track: u32, padding: Option<i64>) -> Vec<u8> {
        let mut payload = block(track);
        if let Some(ticks) = padding {
            payload.extend_from_slice(&element(DISCARD_PADDING, &ticks.to_be_bytes()));
        }
        element(BLOCK_GROUP, &payload)
    }

    /// A whole file: the header the walk sniffs for, then a segment holding `children`.
    fn matroska(children: &[Vec<u8>]) -> Vec<u8> {
        let mut out = element(EBML_HEADER, &[0x00]);
        out.extend_from_slice(&element(SEGMENT, &children.concat()));
        out
    }

    /// The bytes, through a real file, since the walk seeks rather than parsing a slice.
    fn walked(bytes: &[u8]) -> Result<Vec<Discard>, AppError> {
        let tmp = tempfile::TempDir::new()?;
        let path = tmp.path().join("synthetic.mka");
        std::fs::write(&path, bytes)?;
        Ok(discard_padding(&mut File::open(&path)?))
    }

    /// A group earlier in the cluster can state one too, and the encoder's tail is on the last
    /// block rather than the first.
    #[test]
    fn the_last_block_to_state_a_padding_is_the_one_that_answers() -> Result<(), AppError> {
        let groups = [block_group(1, Some(111)), block_group(1, Some(fixture_padding()))].concat();
        let file = matroska(&[element(CLUSTER, &groups)]);

        assert_eq!(only_discard(&walked(&file)?)?, (1, FIXTURE_TICKS));
        Ok(())
    }

    /// A group carrying no padding is not a statement of zero, so it leaves whatever came before it
    /// standing rather than clearing it.
    #[test]
    fn a_group_that_states_no_padding_leaves_the_one_before_it_standing() -> Result<(), AppError> {
        let groups = [block_group(1, Some(fixture_padding())), block_group(1, None)].concat();
        let file = matroska(&[element(CLUSTER, &groups)]);

        assert_eq!(only_discard(&walked(&file)?)?, (1, FIXTURE_TICKS));
        Ok(())
    }

    /// A cluster holds a few seconds, so a file of any length carries several and the tail is in
    /// the final one.
    #[test]
    fn the_last_cluster_is_the_one_read() -> Result<(), AppError> {
        let first = element(CLUSTER, &block_group(1, Some(111)));
        let last = element(CLUSTER, &block_group(1, Some(fixture_padding())));

        assert_eq!(only_discard(&walked(&matroska(&[first, last]))?)?, (1, FIXTURE_TICKS));
        Ok(())
    }

    /// A muxer writing to a pipe leaves its clusters unsized, and a walk stepping over payloads by
    /// size cannot get past one. Answering out of the cluster before it would hand back some other
    /// block's padding as this file's tail.
    #[test]
    fn a_cluster_of_unknown_size_leaves_the_file_stating_nothing() -> Result<(), AppError> {
        let sized = element(CLUSTER, &block_group(1, Some(fixture_padding())));
        let from_a_pipe = open_ended(CLUSTER, &block_group(1, Some(222)));

        assert!(walked(&matroska(&[sized, from_a_pipe]))?.is_empty());
        Ok(())
    }

    /// A negative value states padding at the *start* of the block, which is a splice rather than
    /// the encoder's tail: acting on it would drop audio from the wrong end of the file.
    #[test]
    fn a_padding_stated_at_the_head_of_a_block_is_not_a_tail() -> Result<(), AppError> {
        let file = matroska(&[element(CLUSTER, &block_group(1, Some(-fixture_padding())))]);

        assert!(walked(&file)?.is_empty());
        Ok(())
    }

    /// Both halves are required: a padding naming no track cannot be matched against the one being
    /// decoded.
    #[test]
    fn a_block_group_that_names_no_track_states_nothing() -> Result<(), AppError> {
        let padding = element(DISCARD_PADDING, &fixture_padding().to_be_bytes());
        let file = matroska(&[element(CLUSTER, &element(BLOCK_GROUP, &padding))]);

        assert!(walked(&file)?.is_empty());
        Ok(())
    }

    /// A size field is the file's own choosing, so one claiming more than its parent holds has to
    /// stop the walk rather than send it reading into whatever follows.
    #[test]
    fn an_element_reaching_past_its_parent_is_refused() -> Result<(), AppError> {
        let mut overlong = id_bytes(BLOCK_GROUP);
        overlong.extend_from_slice(&size_vint(u64::from(u32::MAX)));
        let file = matroska(&[element(CLUSTER, &overlong)]);

        assert!(walked(&file)?.is_empty());
        Ok(())
    }

    /// The budget only bounds a file claiming a million empty elements, which would otherwise cost
    /// a seek and a read per two bytes of it.
    #[test]
    fn a_file_of_empty_elements_is_given_up_on() -> Result<(), AppError> {
        let mut file = element(EBML_HEADER, &[0x00]);
        for _ in 0..MAX_ELEMENT_HEADERS {
            // Two bytes apiece: an id, and a one-byte size stating an empty payload.
            file.extend_from_slice(&[0xEC, 0x80]);
        }
        let cluster = element(CLUSTER, &block_group(1, Some(fixture_padding())));
        file.extend_from_slice(&element(SEGMENT, &cluster));

        assert!(walked(&file)?.is_empty(), "the walk read past its own budget");
        Ok(())
    }

    /// An element the walk does not name still has to be stepped over by its stated size, or the
    /// block group after it is never reached.
    #[test]
    fn an_element_the_walk_does_not_name_is_stepped_over() -> Result<(), AppError> {
        let children = [element(VOID, &[0u8; 8]), block_group(1, Some(fixture_padding()))].concat();
        let file = matroska(&[element(CLUSTER, &children)]);

        assert_eq!(only_discard(&walked(&file)?)?, (1, FIXTURE_TICKS));
        Ok(())
    }

    /// The one shape `test-assets/` carries, rebuilt here, so a builder writing bytes nothing else
    /// would accept fails in this file rather than silently pinning the walk against itself.
    #[test]
    fn the_builder_agrees_with_the_fixture_it_imitates() -> Result<(), AppError> {
        let real = discard_padding(&mut File::open(asset("silence-opus.mka"))?);
        let cluster = element(CLUSTER, &block_group(1, Some(fixture_padding())));
        let built = walked(&matroska(&[cluster]))?;

        assert_eq!(only_discard(&built)?, only_discard(&real)?);
        Ok(())
    }
}

/// The byte primitives under the walk, which no container shape can reach every branch of.
mod primitives {
    use super::super::{element_id, vint, vint_len};

    /// EBML states a width in the leading one-bit, so every width has to read back the same value.
    #[test]
    fn a_vint_of_every_legal_width_reads_back_what_was_written() {
        // The value 1 at each width: a marker bit that walks right, then a trailing one.
        let cases: [(&[u8], usize); 8] = [
            (&[0x81], 1),
            (&[0x40, 0x01], 2),
            (&[0x20, 0x00, 0x01], 3),
            (&[0x10, 0x00, 0x00, 0x01], 4),
            (&[0x08, 0x00, 0x00, 0x00, 0x01], 5),
            (&[0x04, 0x00, 0x00, 0x00, 0x00, 0x01], 6),
            (&[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01], 7),
            (&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01], 8),
        ];

        for (bytes, width) in cases {
            assert_eq!(vint(bytes), Some((Some(1), width)), "width {width}");
        }
    }

    /// The spec's way of stating an unknown size, and it is width-relative: the byte that means
    /// "unknown" at one width is an ordinary value at another.
    #[test]
    fn a_size_with_every_payload_bit_set_states_an_unknown_one() {
        let cases: [(&[u8], usize); 4] = [
            (&[0xFF], 1),
            (&[0x7F, 0xFF], 2),
            (&[0x3F, 0xFF, 0xFF], 3),
            (&[0x1F, 0xFF, 0xFF, 0xFF], 4),
        ];

        for (bytes, width) in cases {
            assert_eq!(vint(bytes), Some((None, width)), "width {width}");
        }
    }

    /// An all-zero first byte states a width wider than EBML allows, and would otherwise read as
    /// one byte longer than the longest legal vint.
    #[test]
    fn a_first_byte_of_zeroes_states_no_width() {
        assert_eq!(vint_len(Some(&0x00)), None);
        assert_eq!(vint_len(None), None);
        assert_eq!(vint_len(Some(&0x80)), Some(1));
        assert_eq!(vint_len(Some(&0x01)), Some(8));
    }

    /// Ids are four bytes at most, and one claiming more is a misread the walk must not follow into
    /// whatever sits after it.
    #[test]
    fn an_element_id_wider_than_four_bytes_is_refused() {
        assert_eq!(element_id(&[0xA0]), Some((0xA0, 1)));
        assert_eq!(element_id(&[0x1A, 0x45, 0xDF, 0xA3]), Some((0x1A45_DFA3, 4)));
        // A leading 0x08 states five bytes, one past what an id may occupy.
        assert_eq!(element_id(&[0x08, 0x00, 0x00, 0x00, 0x01]), None);
    }
}
