//! The one place the mixer's samples become an exclusive backend's bytes, so the bit-perfect claim
//! is checkable against one function. The scaling is the exact inverse of the decoder's power of
//! two; anything the chain changed rounds to nearest and saturates, so full-scale `+1.0` never
//! wraps negative. Writers go through [`BlockEncoder`], which dithers first.

use std::fmt;

use melodia_audio::player::source::audio::{Sample, SourceFormat};

use super::dither::Dither;

/// All little-endian. The three 24-bit layouts are different formats: `S24Packed` is three bytes,
/// `S24Low` the bottom of a 32-bit container (ALSA's `S24_LE`) and `S24High` its top (WASAPI's
/// 24-in-32). Writing one where another was opened plays 48 dB off.
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
    /// Best first: the source's own width, the widest integer holding it, then the 24-in-32
    /// containers, a backend skipping what it can't spell. A float source takes the narrow integers
    /// last rather than none, or a device lacking F32 would send it to the shared mixer instead.
    pub fn ladder(source: SourceFormat) -> &'static [Self] {
        use DeviceFormat::{F32, S16, S24High, S24Low, S24Packed, S32};
        match source {
            SourceFormat { float: false, bits: ..=16, .. } => {
                &[S16, S32, S24Packed, S24Low, S24High]
            }
            SourceFormat { float: false, bits: 17..=24, .. } => &[S24Packed, S32, S24Low, S24High],
            SourceFormat { float: false, .. } => &[S32, F32],
            SourceFormat { float: true, .. } => &[S32, F32, S24Packed, S24Low, S24High, S16],
        }
    }

    /// A float source only in [`Self::F32`]: an `f32` under 2⁻³¹ has bits no 32-bit integer holds.
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

    pub(super) fn integer_bits(self) -> Option<u8> {
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

/// What an exclusive writer turns each block it pulls into: dithered where the chain moved it off
/// the format's grid, then encoded. One type for both steps, so no writer can narrow a block
/// without the dither.
pub struct BlockEncoder {
    format: DeviceFormat,
    dither: Dither,
    bytes: Vec<u8>,
}

impl BlockEncoder {
    /// An encoder into `format` with room for a block of `block_samples`, so a writer on a
    /// real-time thread allocates nothing once it starts.
    pub fn new(format: DeviceFormat, block_samples: usize) -> Self {
        let bytes = Vec::with_capacity(block_samples * format.bytes_per_sample());
        Self { format, dither: Dither::default(), bytes }
    }

    /// `block` as the device's bytes, dithering it in place first.
    pub fn encode(&mut self, block: &mut [Sample]) -> &[u8] {
        self.dither.quantize(block, self.format);
        encode(block, self.format, &mut self.bytes);
        &self.bytes
    }
}

/// Replace `out` with `samples` in `format`, keeping its capacity. Reached only through
/// [`BlockEncoder`], so nothing encodes a block the dither hasn't seen.
fn encode(samples: &[Sample], format: DeviceFormat, out: &mut Vec<u8>) {
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

/// Sign-extended into an `i32`, through `f64`, where `2³¹ − 1` is representable and every step
/// exact. A NaN lands on zero through the cast.
#[expect(clippy::cast_possible_truncation, reason = "clamped to the target width first")]
fn to_integer(sample: Sample, bits: u8) -> i32 {
    let scale = full_scale(bits);
    nearest_step(f64::from(sample) * scale, scale) as i32
}

/// What a `bits`-wide integer counts to at `1.0`, the power of two the decoder divided by.
pub(super) fn full_scale(bits: u8) -> f64 {
    f64::from(1_u32 << (bits - 1))
}

/// The integer nearest `level`, saturated to `-scale..scale`.
pub(super) fn nearest_step(level: f64, scale: f64) -> f64 {
    level.round().clamp(-scale, scale - 1.0)
}

#[cfg(test)]
#[path = "tests/encode_tests.rs"]
mod tests;
