//! Opus over `opus-pure`, registered into [`super::decode::CODECS`], Symphonia demuxing it and
//! shipping no decoder (`docs/adr/0040-opus-decodes-through-a-pure-rust-crate.md`). Ogg, Matroska
//! and MP4 all hand the identification packet over whole, so the container is invisible from here.
//!
//! Symphonia drops two of that packet's numbers, both read back below: the playback gain, and the
//! pre-skip. **The priming comes off here** rather than in [`super::file_decode`], which covers
//! all three containers and leaves only a timeline offset to take back. **The stream layout is ours
//! to check**: neither crate reads the mapping table, so a file disagreeing with the canonical
//! layout would decode into the wrong channels in silence.
//!
//! **No soft clip**: the output rings slightly past ±1, as libopus does, and the chain clamps
//! whenever anything is enabled, so a nonlinearity on every Opus track is the wrong trade.

use std::time::Duration;

use opus_pure::{ChannelLayout, MAX_PACKET_SAMPLES};
use symphonia::core::audio::{
    AsGenericAudioBufferRef, AudioBuffer, AudioMut, AudioSpec, Channels, GenericAudioBufferRef,
    Position,
};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{RegisterableAudioDecoder, SupportedAudioCodec};
use symphonia::core::errors::{
    Error as SymphoniaError, Result as SymphoniaResult, decode_error, unsupported_error,
};
use symphonia::core::packet::PacketRef;

/// The rate Opus decodes at, whichever rate the encoder was fed. `OpusHead` states that input
/// rate too, and it describes nothing about playback, which is why nothing here reads it.
const DECODE_RATE: u32 = 48_000;

/// Spelled out rather than through `support_audio_codec!`, which hard-codes a
/// `symphonia_core::` path this workspace cannot resolve: it depends on the facade alone.
static SUPPORTED_CODECS: [SupportedAudioCodec; 1] = [SupportedAudioCodec {
    id: CODEC_ID_OPUS,
    info: CodecInfo { short_name: "opus", long_name: "Opus", profiles: &[] },
}];

/// Decoded and thrown away ahead of a seek target, per RFC 7845 section 4.2: Opus carries state
/// across packets, so a reset decoder opens on audio that is wrong rather than merely different.
/// Usually free, [`super::file_decode`] already discarding from the packet the demuxer landed in.
pub(super) const SEEK_PRE_ROLL: Duration = Duration::from_millis(80);

/// Nothing for every other codec: MP3, AAC and Vorbis hold state too, but widening their seeks is
/// a change each has to earn on its own evidence.
pub(super) fn seek_pre_roll(params: &AudioCodecParameters) -> Duration {
    if params.codec == CODEC_ID_OPUS { SEEK_PRE_ROLL } else { Duration::ZERO }
}

/// What the identification packet says that reaches this decoder and nowhere else.
#[derive(Clone, Copy, Default)]
struct Head {
    /// Frames of encoder priming at 48 kHz, ahead of the first real sample.
    pre_skip: u16,
    /// Playback gain in Q7.8 dB, which RFC 7845 section 5.1 asks be applied by default.
    gain_q8: i16,
    /// Which channel order the stream is coded in: 0 for mono and stereo, 1 for the Vorbis
    /// surround orders. Symphonia refuses everything above before the packet reaches here.
    mapping_family: u8,
}

impl Head {
    /// Offsets rather than a parse, RFC 7845 section 5.1 having frozen the layout and Symphonia
    /// having validated the header. Too short states none, and family 0 is the right default.
    fn read(extra_data: Option<&[u8]>) -> Self {
        /// From the `OpusHead` magic, which every container keeps at the front of `extra_data`.
        const PRE_SKIP: usize = 10;
        const OUTPUT_GAIN: usize = 16;
        const MAPPING_FAMILY: usize = 18;

        let Some(data) = extra_data else {
            return Self::default();
        };
        let (Some(pre_skip), Some(gain)) = (le_u16(data, PRE_SKIP), le_u16(data, OUTPUT_GAIN))
        else {
            return Self::default();
        };
        let mapping_family = data.get(MAPPING_FAMILY).copied().unwrap_or_default();
        Self { pre_skip, gain_q8: gain.cast_signed(), mapping_family }
    }
}

/// Refuses a header stating a stream layout other than the one `layout` was built as. Family 0
/// states none.
fn check_stated_layout(extra_data: Option<&[u8]>, layout: &ChannelLayout) -> SymphoniaResult<()> {
    /// Straight after the mapping family, which only a non-zero one is followed by.
    const STREAM_COUNT: usize = 19;
    const COUPLED_COUNT: usize = 20;
    const MAPPING: usize = 21;

    if layout.mapping_family == 0 {
        return Ok(());
    }

    let data = extra_data.unwrap_or_default();
    let (Some(&streams), Some(&coupled), Some(mapping)) = (
        data.get(STREAM_COUNT),
        data.get(COUPLED_COUNT),
        data.get(MAPPING..MAPPING + layout.nb_channels),
    ) else {
        // Ogg admits a 19-byte packet and MP4 an 11-byte `dOps`, both stopping past the family.
        return unsupported_error("opus: header states no channel mapping");
    };

    if usize::from(streams) != layout.nb_streams
        || usize::from(coupled) != layout.nb_coupled_streams
        || mapping != layout.mapping
    {
        return unsupported_error("opus: header states a channel mapping that is not this one");
    }
    Ok(())
}

/// RFC 7845 section 5.1.1.2's Vorbis orders, in the order Opus hands channels over. A buffer's
/// planes run in ascending position instead, which puts LFE fourth where Opus puts it last.
const VORBIS_ORDER: [&[Position]; 8] = [
    &[Position::FRONT_LEFT],
    &[Position::FRONT_LEFT, Position::FRONT_RIGHT],
    &[Position::FRONT_LEFT, Position::FRONT_CENTER, Position::FRONT_RIGHT],
    &[Position::FRONT_LEFT, Position::FRONT_RIGHT, Position::REAR_LEFT, Position::REAR_RIGHT],
    &[
        Position::FRONT_LEFT,
        Position::FRONT_CENTER,
        Position::FRONT_RIGHT,
        Position::REAR_LEFT,
        Position::REAR_RIGHT,
    ],
    &[
        Position::FRONT_LEFT,
        Position::FRONT_CENTER,
        Position::FRONT_RIGHT,
        Position::REAR_LEFT,
        Position::REAR_RIGHT,
        Position::LFE1,
    ],
    &[
        Position::FRONT_LEFT,
        Position::FRONT_CENTER,
        Position::FRONT_RIGHT,
        Position::SIDE_LEFT,
        Position::SIDE_RIGHT,
        Position::REAR_CENTER,
        Position::LFE1,
    ],
    &[
        Position::FRONT_LEFT,
        Position::FRONT_CENTER,
        Position::FRONT_RIGHT,
        Position::SIDE_LEFT,
        Position::SIDE_RIGHT,
        Position::REAR_LEFT,
        Position::REAR_RIGHT,
        Position::LFE1,
    ],
];

/// The positioned set `count` channels carry, and which Opus output channel feeds each plane.
/// Derived rather than read off the container, Matroska reporting `Channels::Discrete` with no
/// positions at all; Ogg and MP4 build theirs from this same table, so nothing is overridden.
fn positioned_layout(count: usize) -> Option<(Channels, Box<[usize]>)> {
    let order = *VORBIS_ORDER.get(count.checked_sub(1)?)?;
    let channels =
        Channels::Positioned(order.iter().copied().fold(Position::empty(), |set, pos| set | pos));

    let mut sources = vec![0; order.len()];
    for (channel, &pos) in order.iter().enumerate() {
        sources[channels.get_canonical_index_for_positioned_channel(pos)?] = channel;
    }
    Some((channels, sources.into_boxed_slice()))
}

/// The little-endian `u16` at `at`, or `None` where the slice ends before it.
fn le_u16(data: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

/// Saturating, so a count too large to represent empties the packet rather than wrapping.
fn trim_count(frames: u64) -> usize {
    usize::try_from(frames).unwrap_or(usize::MAX)
}

/// Opus over `opus_pure`, for the `CODEC_ID_OPUS` tracks Symphonia's own registry cannot serve.
pub struct OpusDecoder {
    params: AudioCodecParameters,
    decoder: opus_pure::OpusMSDecoder,
    buf: AudioBuffer<f32>,
    /// Sized once for the longest packet Opus allows, so nothing here grows; warm `opus_pure`
    /// allocates per packet only on a SILK/CELT switch.
    pcm: Vec<f32>,
    channels: usize,
    /// Which Opus output channel feeds each plane of [`Self::buf`].
    sources: Box<[usize]>,
    /// Spent across as many packets as it takes, a cropped stream stating at least 3840 frames.
    /// [`AudioDecoder::reset`] doesn't re-arm it, so a seek can't cut real audio.
    pre_skip: usize,
    /// Off, the priming stays in under [`super::file_decode`]'s timeline offset; on by default.
    gapless: bool,
}

impl OpusDecoder {
    fn try_new(params: &AudioCodecParameters, opts: AudioDecoderOptions) -> SymphoniaResult<Self> {
        let Some(count) = params.channels.as_ref().map(Channels::count) else {
            return unsupported_error("opus: channels or a channel layout is required");
        };
        let Some((channels, sources)) = positioned_layout(count) else {
            return unsupported_error("opus: channel count is not one Opus states an order for");
        };

        let head = Head::read(params.extra_data.as_deref());
        // Multistream for every count, family 0 being one stream with an identity mapping, so the
        // counts a family can't carry are `ChannelLayout::surround`'s to refuse.
        let mut decoder =
            opus_pure::OpusMSDecoder::new(DECODE_RATE.cast_signed(), count, head.mapping_family)
                .map_err(|_| SymphoniaError::Unsupported("opus: stream cannot be decoded"))?;
        check_stated_layout(params.extra_data.as_deref(), decoder.layout())?;

        // RFC 7845 asks this be applied by default, so it belongs here rather than beside
        // ReplayGain, where the user's toggle would gate it. It describes the file, not a stream.
        for stream in decoder.streams_mut() {
            stream.gain_q8 = i32::from(head.gain_q8);
        }

        Ok(Self {
            params: params.clone(),
            decoder,
            buf: AudioBuffer::new(AudioSpec::new(DECODE_RATE, channels), MAX_PACKET_SAMPLES),
            pcm: vec![0.0; MAX_PACKET_SAMPLES * count],
            channels: count,
            sources,
            pre_skip: usize::from(head.pre_skip),
            gapless: opts.gapless,
        })
    }

    fn decode_inner(&mut self, packet: &PacketRef<'_>) -> SymphoniaResult<()> {
        let frames = self
            .decoder
            .decode(packet.data, MAX_PACKET_SAMPLES, &mut self.pcm)
            .map_err(|_| SymphoniaError::DecodeError("opus: packet could not be decoded"))?;

        // `render_uninit` asserts rather than failing, and this runs on the audio callback
        // thread, where a panic is the whole process under `panic = "abort"`.
        if frames > self.buf.capacity() {
            return decode_error("opus: packet decoded longer than Opus allows");
        }

        let decoded = &self.pcm[..frames * self.channels];
        self.buf.clear();
        self.buf.render_uninit(Some(frames));

        // Through the plane order, `copy_from_slice_interleaved` assuming the two orders agree.
        let channels = self.channels;
        for (plane, &source) in self.buf.iter_planes_mut().zip(&self.sources) {
            for (sample, &value) in
                plane.iter_mut().zip(decoded.iter().skip(source).step_by(channels))
            {
                *sample = value;
            }
        }

        if self.gapless {
            let start = trim_count(packet.trim_start.get());
            let end = trim_count(packet.trim_end.get());
            // `AudioBuffer::trim` clears the buffer on an overshoot and reports nothing back, so a
            // pre-skip outrunning one packet has to carry its own remainder or the rest plays.
            let available = frames.saturating_sub(start).saturating_sub(end);
            self.buf.trim(start.saturating_add(self.pre_skip), end);
            self.pre_skip = self.pre_skip.saturating_sub(available);
        }
        Ok(())
    }
}

impl AudioDecoder for OpusDecoder {
    fn reset(&mut self) {
        // No failing path; one that refused would keep its state, an artefact rather than silence.
        let _ = self.decoder.reset_state();
    }

    fn codec_info(&self) -> &CodecInfo {
        &SUPPORTED_CODECS[0].info
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> SymphoniaResult<GenericAudioBufferRef<'_>> {
        match self.decode_inner(packet) {
            Ok(()) => Ok(self.buf.as_generic_audio_buffer_ref()),
            Err(e) => {
                // The trait asks that a failed decode leave `last_decoded` empty.
                self.buf.clear();
                Err(e)
            }
        }
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.buf.as_generic_audio_buffer_ref()
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(
        params: &AudioCodecParameters,
        opts: &AudioDecoderOptions,
    ) -> SymphoniaResult<Box<dyn AudioDecoder>>
    where
        Self: Sized,
    {
        Ok(Box::new(Self::try_new(params, *opts)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        &SUPPORTED_CODECS
    }
}

#[cfg(test)]
#[path = "tests/opus_tests.rs"]
mod tests;
