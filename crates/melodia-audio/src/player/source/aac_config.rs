//! Demoting an HE-AAC config to the AAC-LC core layer it already describes.
//!
//! Symphonia 0.6 refuses an AAC track whose config declares SBR, where 0.5 decoded the LC core and
//! dropped SBR and PS, so an HE-AAC v2 file came out a half-rate mono core. Rewriting the config
//! keeps such a file playing at exactly that and nothing better, and retires itself if upstream
//! gains HE-AAC.
//!
//! As narrow as the gate: only a config *opening* at object type 5 or 29 raises it. ADTS
//! synthesises a config with SBR clear, but fragmented MP4 reaches here off a station as well as a
//! file, hence one call in [`super::decode::make_decoder`].

use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;

/// Audio object types, as MPEG-4 numbers them.
const OBJECT_TYPE_LC: u32 = 2;
const OBJECT_TYPE_SBR: u32 = 5;
const OBJECT_TYPE_PS: u32 = 29;
/// Object types from 32 up are spelled as this escape plus six more bits.
const OBJECT_TYPE_ESCAPE: u32 = 31;

/// A sampling frequency index of 15 means the rate follows verbatim in 24 bits.
const FREQUENCY_INDEX_ESCAPE: u32 = 15;
/// The last index the MPEG-4 rate table defines; 13 and 14 are reserved.
const FREQUENCY_INDEX_MAX: u32 = 12;
/// The last channel configuration naming a layout; 0 defers to a program config element.
const CHANNEL_CONFIG_MAX: u32 = 7;

/// `frameLengthFlag`, within the three bits of `GASpecificConfig` that close the config.
const FRAME_LENGTH_960: u32 = 0b100;

/// A sampling frequency as the config spells it, so it can be written back unchanged.
enum SamplingFrequency {
    Index(u32),
    Explicit(u32),
}

/// Every other codec, and every AAC config with nothing to demote, is left exactly as it was.
pub(super) fn demote_he_aac(params: &mut AudioCodecParameters) {
    if params.codec != CODEC_ID_AAC {
        return;
    }
    if let Some(asc) = params.extra_data.as_deref()
        && let Some(core) = core_layer_config(asc)
    {
        log::info!("HE-AAC: only its AAC-LC core is decoded, so half rate, and mono for HE-AAC v2");
        params.extra_data = Some(core);
    }
}

/// `None` when there is nothing to demote.
fn core_layer_config(asc: &[u8]) -> Option<Box<[u8]>> {
    let mut bits = BitReader::new(asc);

    if !matches!(read_object_type(&mut bits)?, OBJECT_TYPE_SBR | OBJECT_TYPE_PS) {
        return None;
    }

    // Both are read ahead of the SBR branch, so both are already the core layer's exactly.
    let frequency = read_sampling_frequency(&mut bits)?;
    let channel_config = bits.read(4)?;
    if channel_config == 0 || channel_config > CHANNEL_CONFIG_MAX {
        return None;
    }

    // SBR's own rate is the core's doubled, and a decoder that drops SBR has no use for it.
    read_sampling_frequency(&mut bits)?;
    if read_object_type(&mut bits)? != OBJECT_TYPE_LC {
        return None;
    }

    // The GASpecificConfig. A 960-frame core stays refused, 0.5 refusing it too; the other two
    // flags go back as zero, the only shape an AAC-LC core has. All three must be *present*, a
    // config too short for them being one Symphonia refuses ahead of the gate.
    if bits.read(3)? & FRAME_LENGTH_960 != 0 {
        return None;
    }

    let mut core = BitWriter::default();
    core.write(OBJECT_TYPE_LC, 5);
    match frequency {
        SamplingFrequency::Index(index) => core.write(index, 4),
        SamplingFrequency::Explicit(hz) => {
            core.write(FREQUENCY_INDEX_ESCAPE, 4);
            core.write(hz, 24);
        }
    }
    core.write(channel_config, 4);
    core.write(0, 3); // GASpecificConfig: 1024 frames, no core dependency, no extension.
    Some(core.finish())
}

fn read_object_type(bits: &mut BitReader<'_>) -> Option<u32> {
    match bits.read(5)? {
        OBJECT_TYPE_ESCAPE => Some(bits.read(6)? + 32),
        object_type => Some(object_type),
    }
}

fn read_sampling_frequency(bits: &mut BitReader<'_>) -> Option<SamplingFrequency> {
    match bits.read(4)? {
        FREQUENCY_INDEX_ESCAPE => Some(SamplingFrequency::Explicit(bits.read(24)?)),
        index if index <= FREQUENCY_INDEX_MAX => Some(SamplingFrequency::Index(index)),
        _ => None,
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit: 0 }
    }

    /// The next `count` bits, most significant first, or `None` once the config runs out.
    fn read(&mut self, count: u32) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            let byte = *self.bytes.get(self.bit / 8)?;
            value = (value << 1) | u32::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Some(value)
    }
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    /// Appends the low `count` bits of `value`, most significant first.
    fn write(&mut self, value: u32, count: u32) {
        for shift in (0..count).rev() {
            if self.bit.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if (value >> shift) & 1 == 1
                && let Some(byte) = self.bytes.last_mut()
            {
                *byte |= 0x80 >> (self.bit % 8);
            }
            self.bit += 1;
        }
    }

    /// The finished config, its part-filled last byte reading as the zero padding a decoder
    /// expects to find.
    fn finish(self) -> Box<[u8]> {
        self.bytes.into_boxed_slice()
    }
}

#[cfg(test)]
#[path = "tests/aac_config_tests.rs"]
mod tests;
