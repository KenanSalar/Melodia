//! Turning the mixer's samples into the bytes an owned backend writes to the device.
//!
//! **This is the only place that happens.** cpal owns the byte layout on the shared path; an
//! exclusive backend hands the card raw bytes, and every one of them is produced here, so the
//! bit-perfect claim is checkable against one function.
//!
//! The scaling is the exact inverse of the decoder's: Symphonia divides an integer sample by a
//! power of two on the way to `f32`, and multiplying back by the same power lands on the same
//! integer. Anything the chain changed rounds to nearest and saturates, so a full-scale `+1.0`
//! becomes the positive maximum rather than wrapping to the negative one.

use std::fmt;

use melodia_audio::player::source::audio::{Sample, SourceFormat};

/// A sample layout a device can be opened with, all little-endian.
///
/// **The three 24-bit layouts are different formats, not three spellings of one.** `S24Packed`
/// is three bytes per sample and is what many USB DACs offer alone. The other two put 24 bits in
/// a 32-bit container: `S24Low` at the bottom, as ALSA's `S24_LE` does, and `S24High` at the
/// top, as WASAPI's 24-in-32 does. Writing one where the other was opened plays 48 dB off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFormat {
    S16,
    S24Packed,
    S24Low,
    S24High,
    S32,
    F32,
}

impl DeviceFormat {
    /// The formats to try for a source, best first: its own width, then the widest integer
    /// that holds it, then the 24-in-32 containers. A backend skips whichever of those it has no
    /// spelling for.
    pub fn ladder(source: SourceFormat) -> &'static [Self] {
        use DeviceFormat::{F32, S16, S24High, S24Low, S24Packed, S32};
        match source {
            SourceFormat { float: false, bits: ..=16 } => &[S16, S32, S24Packed, S24Low, S24High],
            SourceFormat { float: false, bits: 17..=24 } => &[S24Packed, S32, S24Low, S24High],
            SourceFormat { .. } => &[S32, F32],
        }
    }

    /// Whether a source in `source`'s format reaches the device in this one unchanged.
    ///
    /// A float source only in [`Self::F32`]: an `f32` smaller than 2⁻³¹ has bits a 32-bit
    /// integer cannot hold, so "close enough" would make the panel's claim false.
    pub fn carries(self, source: SourceFormat) -> bool {
        match self.integer_bits() {
            None => source.float && source.bits <= 32,
            Some(bits) => !source.float && source.bits <= bits,
        }
    }

    /// Bytes one sample occupies on the wire.
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::S24Packed => 3,
            Self::S24Low | Self::S24High | Self::S32 | Self::F32 => 4,
        }
    }

    fn integer_bits(self) -> Option<u8> {
        match self {
            Self::S16 => Some(16),
            Self::S24Packed | Self::S24Low | Self::S24High => Some(24),
            Self::S32 => Some(32),
            Self::F32 => None,
        }
    }

    /// How far the value sits above the bottom of its container.
    fn msb_padding(self) -> u32 {
        match self {
            Self::S24High => 8,
            _ => 0,
        }
    }
}

/// The ALSA spelling, which is what `/proc/asound/…/hw_params` prints beside it. ALSA opens
/// `S24High` as `S32_LE` with 24 significant bits, so that is how it reads.
impl fmt::Display for DeviceFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::S16 => "S16_LE",
            Self::S24Packed => "S24_3LE",
            Self::S24Low => "S24_LE",
            Self::S24High => "S32_LE (24 valid)",
            Self::S32 => "S32_LE",
            Self::F32 => "FLOAT_LE",
        })
    }
}

/// Replace `out` with `samples` in `format`. `out` keeps its capacity, so a writer that reuses it
/// allocates only on its first block.
pub fn encode(samples: &[Sample], format: DeviceFormat, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len() * format.bytes_per_sample());
    let Some(bits) = format.integer_bits() else {
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        return;
    };
    // Every integer layout is the low bytes of the sign-extended value: two for `S16`, three for
    // the packed 24, all four for the rest. `S24High` shifts it up first, so it rounds at 24 bits
    // and leaves the low byte clear.
    let width = format.bytes_per_sample();
    let padding = format.msb_padding();
    for &sample in samples {
        let value = to_integer(sample, bits) << padding;
        out.extend_from_slice(&value.to_le_bytes()[..width]);
    }
}

/// `sample` as a signed `bits`-wide integer, sign-extended into an `i32`.
///
/// `f64` because every step is exact there: the scale is a power of two and a 32-bit bound is
/// representable, which `f32` can't say of `2³¹ − 1`. A NaN lands on zero through the cast.
#[expect(clippy::cast_possible_truncation, reason = "clamped to the target width first")]
fn to_integer(sample: Sample, bits: u8) -> i32 {
    let scale = f64::from(1_u32 << (bits - 1));
    (f64::from(sample) * scale).round().clamp(-scale, scale - 1.0) as i32
}

#[cfg(test)]
#[path = "tests/encode_tests.rs"]
mod tests;
