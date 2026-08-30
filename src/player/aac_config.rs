//! Demoting an HE-AAC audio specific config to the AAC-LC core layer it already describes.
//!
//! Symphonia 0.6 refuses an AAC track whose config declares SBR, on the reasonable grounds that
//! a loud "unsupported" beats silently handing back a worse version of the track. 0.5, which
//! rodio pins, never consulted that flag: the same complexity gate passed, the AAC-LC core
//! decoded, and SBR and PS were thrown away — so an HE-AAC v2 file came out as a half rate mono
//! core. Rewriting the config restores exactly that and nothing better. It is what keeps a file
//! that plays today from going silent when local decode moves onto 0.6, not a fix, and it
//! retires itself if upstream's real HE-AAC support lands.
//!
//! The trigger is narrow deliberately, and it is exactly as narrow as the gate. 0.6 raises
//! `sbr_present` only from a config that *opens* at object type 5 or 29; the backward compatible
//! signalling that hides an SBR extension in the config's tail is read only once that branch has
//! run, so those files pass the gate untouched and are none of this module's business.
//!
//! Only containers carrying a real config reach here, so MP4 and Matroska. ADTS synthesises one
//! with SBR clear, which is why the radio work never turned this up — though a station serving
//! fragmented MP4 reaches it all the same, which is why it is applied once in
//! [`super::decode::make_decoder`] rather than at either decoder.

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

/// Replaces an HE-AAC config on `params` with the AAC-LC core layer it describes.
///
/// Every other codec, and every AAC config with nothing to demote, is left exactly as it was.
/// Call it on a track's parameters ahead of building a decoder from them — either decoder can be
/// handed the config 0.6 refuses.
pub(crate) fn demote_he_aac(params: &mut AudioCodecParameters) {
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

/// Rewrites an audio specific config that declares SBR into the plain AAC-LC config its own core
/// layer describes, or `None` when there is nothing to demote.
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

    // The GASpecificConfig. Only frameLengthFlag is consulted, 0.5 having rejected a 960-frame
    // core as well, so demoting one would start a file playing rather than keep it playing, and
    // play it wrong. The other two go back as zero, the only shape an AAC-LC core has. All three
    // must be *present* though: a config too short to hold them is one Symphonia's reader refuses
    // ahead of the gate this exists to get past.
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
