//! Tests for the HE-AAC config demotion.
//!
//! The byte literals are audio specific configs written out field by field; each comment names
//! the fields in the order they are packed. The last test is the point of the module: it asks
//! Symphonia's own decoder, which is the thing that refuses these files.

use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};

use super::core_layer_config;
use crate::error::AppError;

/// Object type 5 (SBR), core 22050 (index 7), stereo, extension 44100 (index 4), core type 2.
const HE_AAC: [u8; 4] = [0x2B, 0x92, 0x08, 0x00];
/// The same with object type 29 (PS) over a mono core, which is HE-AAC v2.
const HE_AAC_V2: [u8; 4] = [0xEB, 0x8A, 0x08, 0x00];
/// Object type 2, 44100, stereo — a plain AAC-LC config, with no SBR to demote.
const AAC_LC: [u8; 2] = [0x12, 0x10];

#[test]
fn a_plain_aac_lc_config_is_left_alone() {
    assert_eq!(core_layer_config(&AAC_LC), None);
}

#[test]
fn an_sbr_config_demotes_to_its_core_layer() {
    // Object type 2, the core's own 22050 and stereo, and an empty GASpecificConfig.
    assert_eq!(core_layer_config(&HE_AAC).as_deref(), Some(&[0x13, 0x90][..]));
}

#[test]
fn a_parametric_stereo_config_keeps_the_mono_core() {
    assert_eq!(core_layer_config(&HE_AAC_V2).as_deref(), Some(&[0x13, 0x88][..]));
}

#[test]
fn an_explicit_sampling_frequency_is_carried_over() {
    // Frequency index 15, so the core rate is spelled out in the 24 bits that follow.
    let escaped = [0x2F, 0x80, 0x2B, 0x11, 0x12, 0x08, 0x00];
    assert_eq!(core_layer_config(&escaped).as_deref(), Some(&[0x17, 0x80, 0x2B, 0x11, 0x10][..]));
}

#[test]
fn a_config_that_0_5_refused_is_not_demoted() {
    // Each of these passes the SBR branch and fails on a term 0.5 also gated on, so demoting it
    // would start a file playing that never played, rather than keep one playing.
    let refused = [
        ("960-frame core", [0x2B, 0x92, 0x0A, 0x00]),
        ("core object type 4, not LC", [0x2B, 0x92, 0x10, 0x00]),
        ("channel configuration 0", [0x2B, 0x82, 0x08, 0x00]),
    ];

    for (what, asc) in refused {
        assert_eq!(core_layer_config(&asc), None, "{what}");
    }
}

#[test]
fn a_truncated_config_is_refused_rather_than_read_past() {
    for len in 0..HE_AAC.len() {
        assert_eq!(core_layer_config(&HE_AAC[..len]), None, "{len} bytes");
    }
}

#[test]
fn demoting_twice_changes_nothing() -> Result<(), AppError> {
    assert_eq!(core_layer_config(&demoted(&HE_AAC)?), None);
    Ok(())
}

#[test]
fn symphonia_refuses_the_config_and_accepts_its_core_layer() -> Result<(), AppError> {
    for (what, asc, rate, channels) in [
        ("HE-AAC", HE_AAC, 22050, 2),
        ("HE-AAC v2", HE_AAC_V2, 22050, 1),
    ] {
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_AAC).with_extra_data(asc.into());
        assert!(
            make_decoder(&params).is_err(),
            "{what}: 0.6 is expected to refuse a config declaring SBR"
        );

        params.extra_data = Some(demoted(&asc)?);
        let decoder = make_decoder(&params).map_err(|e| {
            AppError::Player(format!("{what}: the demoted config should decode: {e}"))
        })?;

        let decoded = decoder.codec_params();
        assert_eq!(decoded.sample_rate, Some(rate), "{what}");
        assert_eq!(decoded.channels.as_ref().map(Channels::count), Some(channels), "{what}");
    }
    Ok(())
}

fn demoted(asc: &[u8]) -> Result<Box<[u8]>, AppError> {
    core_layer_config(asc).ok_or_else(|| AppError::Player("an SBR config should demote".to_owned()))
}

fn make_decoder(
    params: &AudioCodecParameters,
) -> Result<Box<dyn AudioDecoder>, symphonia::core::errors::Error> {
    symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())
}
