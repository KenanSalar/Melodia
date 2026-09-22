//! Tests for the Opus decoder.
//!
//! Counts rather than samples, wherever the question is timing: the audio either side of a
//! pre-skip or end-trim regression is still the right audio, so a comparison against a reference
//! decode would agree on everything it looked at and disagree only on how much there was. The
//! seeks ask the same of the demuxer's timeline against what the decoder hands over.
//!
//! Surround is the one question a count cannot answer, every channel being the same length as
//! every other. It is asked as energy per channel per window instead, off a fixture built so that
//! the answer is the identity and any reorder defect is a visible permutation of it.

use std::time::Duration;

use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::codecs::audio::well_known::{CODEC_ID_FLAC, CODEC_ID_OPUS};

use super::{SEEK_PRE_ROLL, seek_pre_roll};
use crate::player::source::audio::AudioSource;
use crate::player::source::file_decode::FileDecoder;
use crate::player::source::tests::helpers::asset;
use melodia_core::error::AppError;

/// Frames `name` decodes to, pulled to the end.
fn decoded_frames(name: &str) -> Result<usize, AppError> {
    let decoder = FileDecoder::open(&asset(name))?;
    let channels = usize::from(decoder.channels().get());
    Ok(decoder.count() / channels)
}

/// How many frames each of the surround fixture's channels holds its tone for.
const SLOT_FRAMES: usize = 4_800;

/// Which slot each decoded channel is loudest in.
///
/// `surround.opus` gives every channel a tone in a window of its own and silence in the others, so
/// the loudest window names the source channel that plane is carrying. Energy rather than a
/// frequency: it survives a lossy codec and the narrow band libopus gives the LFE stream, where a
/// per-channel pitch does not.
fn loudest_slot_per_channel(name: &str) -> Result<Vec<usize>, AppError> {
    let decoder = FileDecoder::open(&asset(name))?;
    let channels = usize::from(decoder.channels().get());
    let samples: Vec<f32> = decoder.collect();
    let slots = samples.len() / channels / SLOT_FRAMES;

    let energy = |channel: usize, slot: usize| {
        samples
            .iter()
            .skip(slot * SLOT_FRAMES * channels + channel)
            .step_by(channels)
            .take(SLOT_FRAMES)
            .map(|s| s * s)
            .sum::<f32>()
    };
    Ok((0..channels)
        .filter_map(|channel| {
            (0..slots).max_by(|a, b| energy(channel, *a).total_cmp(&energy(channel, *b)))
        })
        .collect())
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

/// Symphonia's own header parse admits mapping family 1 up to eight channels, so a surround file
/// reaches the decoder either way; before it grew one it was turned away there.
#[test]
fn a_surround_file_decodes_to_every_channel_it_states() -> Result<(), AppError> {
    let decoder = FileDecoder::open(&asset("surround.opus"))?;
    assert_eq!(decoder.channels().get(), 6);
    Ok(())
}

/// Opus hands surround channels over in the Vorbis order and a buffer's planes run in ascending
/// channel position, so the two disagree from three channels up: LFE arrives last and is wanted
/// fourth. The identity here is the reorder doing its job — without it this reads as a rotation of
/// the front three with the low-frequency channel in the wrong place, which on a real file is a
/// centre channel playing out of the right speaker.
#[test]
fn surround_channels_land_in_the_plane_order_the_buffer_hands_out() -> Result<(), AppError> {
    assert_eq!(loudest_slot_per_channel("surround.opus")?, [0, 1, 2, 3, 4, 5]);
    Ok(())
}

/// `surround.opus` with its stream count rewritten from 4 to 3 and the Ogg page CRC recomputed, so
/// the container is intact and only the stated layout is wrong. Symphonia reads none of those
/// bytes, and `opus_pure` derives the layout rather than reading them either, so nothing but this
/// check stands between the file and a decode into the wrong channels.
#[test]
fn a_header_disagreeing_with_the_canonical_layout_is_refused() {
    let refused = FileDecoder::open(&asset("surround-bad-layout.opus"));
    assert!(refused.is_err(), "a stream count no 6-channel layout states has to be refused");
}

/// Matroska states a channel count and no order, reporting `Channels::Discrete` for every audio
/// track, so the positioned set is derived from the count rather than read off the container. Read
/// off it, a stereo Opus in an `.mka` does not open at all, and no other fixture carries Opus in a
/// container that answers that way.
#[test]
fn opus_in_a_container_that_names_no_channel_order_still_decodes() -> Result<(), AppError> {
    let decoder = FileDecoder::open(&asset("silence-opus.mka"))?;
    assert_eq!(decoder.channels().get(), 2);
    Ok(())
}

/// The file states 648 frames of `DiscardPadding` and `symphonia-format-mkv` parses that element into
/// a field it reads nowhere, so nothing below [`super::super::mkv_trim`] can tell the decoder to drop
/// them. Without it the tail stays in and the count reads 48648, which is what the reference
/// implementation's own read of the same file says it should not.
#[test]
fn the_padding_a_matroska_file_states_is_never_handed_out() -> Result<(), AppError> {
    assert_eq!(decoded_frames("silence-opus.mka")?, 48_000);
    Ok(())
}

/// The pre-roll is per codec, and the gate is the codec id rather than the presence of a field.
/// Widening every format's seek would change behaviour nothing here has evidence for.
#[test]
fn the_seek_pre_roll_is_opus_and_nothing_else() {
    let mut opus = AudioCodecParameters::new();
    opus.for_codec(CODEC_ID_OPUS);
    let mut flac = AudioCodecParameters::new();
    flac.for_codec(CODEC_ID_FLAC);

    assert_eq!(seek_pre_roll(&opus), SEEK_PRE_ROLL);
    assert_eq!(seek_pre_roll(&flac), Duration::ZERO);
}

/// Nearer the start than the pre-roll is wide, so the ask saturates at zero and the trim has to
/// grow to cover the difference. Landing short here would mean the seek replaying whatever the
/// pre-roll asked for, which at the head of a file is the head of the file.
#[test]
fn a_seek_inside_the_pre_roll_window_still_lands_where_it_asked() -> Result<(), AppError> {
    let mut decoder = FileDecoder::open(&asset("silence.opus"))?;
    decoder.try_seek(Duration::from_millis(20)).map_err(|e| AppError::Player(e.to_string()))?;

    let channels = usize::from(decoder.channels().get());
    assert_eq!(decoder.count() / channels, 47_040);
    Ok(())
}

/// The layout table, the header parse and the refusals under the decode tests above, which no
/// fixture reaches every branch of: `test-assets/` carries a stereo file and a 5.1 one, and the
/// table states eight orders.
mod layout {
    use opus_pure::OpusMSDecoder;
    use symphonia::core::audio::Position;

    use super::super::{DECODE_RATE, Head, check_stated_layout, positioned_layout, trim_count};
    use crate::player::source::file_decode::FileDecoder;
    use crate::player::source::tests::helpers::asset;
    use melodia_core::error::AppError;

    /// Opus states an order for one channel through eight, and nothing either side.
    ///
    /// Zero falls out of the `checked_sub`, which is what a container reporting no channels at all
    /// arrives as; nine is the first count past the table.
    #[test]
    fn a_channel_count_opus_states_no_order_for_is_refused() {
        assert!(positioned_layout(0).is_none(), "no channels is not a layout");
        assert!(positioned_layout(9).is_none(), "and nine is past every order Opus states");
    }

    /// Every count carries as many positions as it claims channels, and each Opus output channel
    /// feeds exactly one plane.
    ///
    /// The permutation is the half worth asserting: the inverse is built by writing each channel
    /// into the slot its position sorts to, so a row naming one position twice silently leaves a
    /// plane reading channel zero and drops the channel that should have fed it.
    #[test]
    fn every_channel_count_opus_states_an_order_for_lands_in_the_plane_order()
    -> Result<(), AppError> {
        for count in 1..=8 {
            let Some((channels, sources)) = positioned_layout(count) else {
                return Err(AppError::Player(format!("{count} channels states no layout")));
            };

            assert_eq!(channels.count(), count, "{count} channels");
            let mut fed: Vec<usize> = sources.to_vec();
            fed.sort_unstable();
            assert_eq!(fed, (0..count).collect::<Vec<_>>(), "{count} channels");
        }
        Ok(())
    }

    /// Opus hands surround over in the Vorbis order and a buffer's planes run in ascending channel
    /// position, so the two part company from three channels up: LFE arrives last and is wanted
    /// fourth. The unit twin of the energy check above, which is what says these indices are the
    /// right way round rather than merely self-consistent.
    #[test]
    fn the_low_frequency_channel_arrives_last_and_is_wanted_fourth() -> Result<(), AppError> {
        let Some((channels, sources)) = positioned_layout(6) else {
            return Err(AppError::Player("5.1 states no layout".to_owned()));
        };
        let Some(plane) = channels.get_canonical_index_for_positioned_channel(Position::LFE1)
        else {
            return Err(AppError::Player("5.1 names no low-frequency channel".to_owned()));
        };

        assert_eq!(plane, 3, "the buffer sorts LFE fourth");
        assert_eq!(sources.get(plane), Some(&5), "and Opus hands it over last");
        Ok(())
    }

    /// Matroska reports `Channels::Discrete` for every audio track, so the positioned set is
    /// derived from the count rather than read off the container. A zero-channel header reaches
    /// here as a count of nothing and has to be turned away rather than decoded into no planes.
    #[test]
    fn a_container_that_states_no_channels_is_refused_rather_than_decoded() {
        let refused = FileDecoder::open(&asset("silence-zero-channels.opus"));

        assert!(refused.is_err(), "a header stating no channels has to be refused");
    }

    /// An `OpusHead` is a frozen layout read at fixed offsets, so a short one states nothing rather
    /// than reading a field out of whatever follows. Family 0 is the right default for that: the
    /// two families build the same layout for the counts family 0 admits.
    #[test]
    fn a_header_too_short_to_state_its_fields_states_none_of_them() {
        // `OpusHead`, version 1, two channels, pre-skip 312, input rate 48k, gain 0, family 0.
        let whole: [u8; 19] = [
            b'O', b'p', b'u', b's', b'H', b'e', b'a', b'd', 1, 2, 0x38, 0x01, 0x80, 0xBB, 0x00,
            0x00, 0x00, 0x00, 0,
        ];

        let read = Head::read(Some(&whole));
        assert_eq!((read.pre_skip, read.gain_q8, read.mapping_family), (312, 0, 0));

        for short in [0usize, 11, 17] {
            let truncated = Head::read(whole.get(..short));
            assert_eq!(
                (truncated.pre_skip, truncated.gain_q8, truncated.mapping_family),
                (0, 0, 0),
                "{short} bytes states a field it does not carry"
            );
        }
        // One byte past the gain and one short of the family, which defaults rather than refusing.
        let no_family = Head::read(whole.get(..18));
        assert_eq!((no_family.pre_skip, no_family.mapping_family), (312, 0));
        assert_eq!(Head::read(None).pre_skip, 0);
    }

    /// The gain is the one field read as signed, and it is the quiet half of the header: a file
    /// asking to play softer reads as one asking to play far louder if the sign is dropped.
    #[test]
    fn a_negative_output_gain_reads_back_as_one() {
        let mut header: [u8; 19] = [
            b'O', b'p', b'u', b's', b'H', b'e', b'a', b'd', 1, 2, 0x38, 0x01, 0x80, 0xBB, 0x00,
            0x00, 0x00, 0x00, 0,
        ];
        // -256 in Q7.8, which is one decibel down.
        header[16] = 0x00;
        header[17] = 0xFF;

        assert_eq!(Head::read(Some(&header)).gain_q8, -256);
    }

    /// Family 0 states no stream layout to disagree with, so the check has nothing to read and must
    /// not refuse a header that legitimately stops at the family byte.
    #[test]
    fn a_family_0_header_states_no_layout_to_disagree_with() -> Result<(), AppError> {
        let stereo = OpusMSDecoder::new(DECODE_RATE.cast_signed(), 2, 0)
            .map_err(|_| AppError::Player("stereo is not a layout opus_pure builds".to_owned()))?;

        assert!(check_stated_layout(None, stereo.layout()).is_ok());
        Ok(())
    }

    /// Ogg admits a 19-byte identification packet and MP4 an 11-byte `dOps` body, both of which
    /// stop one byte past the family, so a family-1 header carrying no mapping table gets as far as
    /// the check. Read past, those bytes are whatever the container put after the header.
    #[test]
    fn a_family_1_header_with_no_mapping_table_is_refused() -> Result<(), AppError> {
        let surround = OpusMSDecoder::new(DECODE_RATE.cast_signed(), 6, 1)
            .map_err(|_| AppError::Player("5.1 is not a layout opus_pure builds".to_owned()))?;
        let stops_at_the_family = [0u8; 19];

        assert!(check_stated_layout(Some(&stops_at_the_family), surround.layout()).is_err());
        Ok(())
    }

    /// The trim it feeds treats anything past the buffer as the whole of it, so a count too large
    /// to represent has to empty the packet rather than wrap to a small one.
    #[test]
    fn a_frame_count_too_large_to_represent_empties_the_packet() {
        assert_eq!(trim_count(0), 0);
        assert_eq!(trim_count(312), 312);
        assert_eq!(trim_count(u64::MAX), usize::MAX);
    }
}
