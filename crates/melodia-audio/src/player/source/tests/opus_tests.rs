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

use std::path::{Path, PathBuf};
use std::time::Duration;

use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::codecs::audio::well_known::{CODEC_ID_FLAC, CODEC_ID_OPUS};

use super::{SEEK_PRE_ROLL, seek_pre_roll};
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
///
/// The frame count is deliberately not asserted: `symphonia-format-mkv` parses `DiscardPadding` and
/// applies it nowhere, so the tail padding stays in where ffmpeg's own read of the same file drops
/// it.
#[test]
fn opus_in_a_container_that_names_no_channel_order_still_decodes() -> Result<(), AppError> {
    let decoder = FileDecoder::open(&asset("silence-opus.mka"))?;
    assert_eq!(decoder.channels().get(), 2);
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
