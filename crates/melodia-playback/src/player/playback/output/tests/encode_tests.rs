//! Tests for the one place samples become device bytes.
//!
//! The round-trip cases take each layout's integer extremes and the values next to zero, divide
//! them the way the decoder does, and expect the encoder to hand the same integer back. That
//! inverse is the whole of the bit-perfect claim below the mixer.

use super::{DeviceFormat, encode};
use melodia_audio::player::source::audio::SourceFormat;

/// `value` as the decoder would hand it over from a `bits`-wide integer sample.
fn decoded(value: i32, bits: u8) -> f32 {
    let scale = f64::from(1_u32 << (bits - 1));
    #[expect(clippy::cast_possible_truncation, reason = "exact for every width up to 24 bits")]
    let sample = (f64::from(value) / scale) as f32;
    sample
}

fn encoded(samples: &[f32], format: DeviceFormat) -> Vec<u8> {
    let mut out = Vec::new();
    encode(samples, format, &mut out);
    out
}

#[test]
fn every_integer_layout_hands_the_decoded_value_back() {
    let cases: [(DeviceFormat, u8, &[i32]); 3] = [
        (DeviceFormat::S16, 16, &[i32::from(i16::MIN), -1, 0, 1, i32::from(i16::MAX)]),
        (DeviceFormat::S24Packed, 24, &[-8_388_608, -1, 0, 1, 8_388_607]),
        (DeviceFormat::S24Low, 24, &[-8_388_608, -1, 0, 1, 8_388_607]),
    ];
    for (format, bits, values) in cases {
        let samples: Vec<f32> = values.iter().map(|&v| decoded(v, bits)).collect();
        let width = format.bytes_per_sample();
        let expected: Vec<u8> =
            values.iter().flat_map(|v| v.to_le_bytes()[..width].to_vec()).collect();

        assert_eq!(encoded(&samples, format), expected, "{format}");
    }
}

/// `S24_LE` is 24 bits in the low end of a 32-bit word, sign-extended; the 48 dB-too-quiet bug is
/// what reading it as MSB-aligned produces.
#[test]
fn s24_low_is_sign_extended_in_the_low_three_bytes() {
    assert_eq!(encoded(&[decoded(-1, 24)], DeviceFormat::S24Low), [0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(encoded(&[decoded(1, 24)], DeviceFormat::S24Low), [0x01, 0x00, 0x00, 0x00]);
}

#[test]
fn full_scale_and_beyond_saturate_rather_than_wrap() {
    let samples = [1.0, 2.0, -1.0, -2.0];
    let s16: Vec<u8> =
        [i16::MAX, i16::MAX, i16::MIN, i16::MIN].iter().flat_map(|v| v.to_le_bytes()).collect();
    let s32: Vec<u8> =
        [i32::MAX, i32::MAX, i32::MIN, i32::MIN].iter().flat_map(|v| v.to_le_bytes()).collect();

    assert_eq!(encoded(&samples, DeviceFormat::S16), s16);
    assert_eq!(encoded(&samples, DeviceFormat::S32), s32);
}

#[test]
fn a_nan_is_written_as_silence() {
    assert_eq!(encoded(&[f32::NAN], DeviceFormat::S16), [0, 0]);
}

/// Float is written as it is, sign of zero included, which is what the mixer keeps it for.
#[test]
fn float_passes_through_bit_for_bit() {
    let samples = [-0.0_f32, 0.123_456_79, -1.5];
    let expected: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

    assert_eq!(encoded(&samples, DeviceFormat::F32), expected);
}

#[test]
fn a_format_carries_a_source_only_when_it_holds_every_bit() {
    let s16 = SourceFormat { bits: 16, float: false };
    let s24 = SourceFormat { bits: 24, float: false };
    let rows = [
        (DeviceFormat::S16, s16, true),
        (DeviceFormat::S16, s24, false),
        (DeviceFormat::S24Packed, s24, true),
        (DeviceFormat::S32, s24, true),
        // A float below 2^-31 has bits no 32-bit integer holds.
        (DeviceFormat::S32, SourceFormat::F32, false),
        (DeviceFormat::F32, SourceFormat::F32, true),
        (DeviceFormat::F32, s16, false),
    ];
    for (format, source, carries) in rows {
        assert_eq!(format.carries(source), carries, "{format} carrying {source:?}");
    }
}
