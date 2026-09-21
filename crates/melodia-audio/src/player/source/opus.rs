//! Opus, which Symphonia demuxes and has no decoder for.
//!
//! 0.6.1 ships none and offers no feature for one, so this is a registration into
//! [`super::decode::CODECS`] over `opus-pure` and nothing at either call site. Ogg is not the
//! only door: Matroska maps `A_OPUS` and MP4 parses `dOps`, and all three hand the
//! identification packet over whole as `extra_data`, so which container it was is invisible
//! from here.
//!
//! **Two numbers Symphonia reads out of that packet and then drops.** The playback gain reaches
//! no field of `AudioCodecParameters` at all, and the pre-skip lands on `Track::delay`, which
//! [`super::decode::open`] never hands a decoder. Both are read back here, off the container's
//! own copy of the header.
//!
//! **The priming comes off here rather than in [`super::file_decode`], which is what covers all
//! three containers**: only Ogg stands a `Track::delay` up for a reader above the codec to find.
//! The cost is that the demuxer's timeline is then ahead of the samples handed out, the Ogg
//! mapper leaving `absgp_to_ts` at identity so the granule position arrives still counting the
//! priming and the first packet's timestamp is zero rather than negative.
//! [`super::file_decode::FileDecoder::install_trim`] takes that offset back off the length and
//! adds it onto a seek, reading the same `Track::delay`, so the two timelines agree again without
//! this module having to know which container it was handed.
//!
//! **The stream layout is ours to check.** Symphonia's header reader stops at the mapping family
//! byte, so the stream count, the coupled count and the mapping table reach nothing above the
//! codec, and `opus_pure` derives its layout from the channel count rather than reading them. A
//! file disagreeing with the canonical layout is refused here or it decodes into the wrong
//! channels in silence.
//!
//! **No soft clip.** The float output rings slightly past plus or minus one, as libopus does,
//! and the DSP chain clamps whenever anything is enabled. A nonlinearity on every Opus track,
//! to correct an overshoot the device converts identically either way, is the wrong trade.

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

/// Audio to decode and throw away ahead of a seek target, per RFC 7845 section 4.2.
///
/// A seek resets the decoder, and Opus carries state across packets that the first ones after a
/// reset have to rebuild, so played from the target itself a seek opens on audio that is wrong
/// rather than merely different. It is a floor and usually free: [`super::file_decode`] already
/// discards everything between the packet the demuxer landed in and the frame asked for, and this
/// only widens that where the landing was too close to warm anything.
pub(super) const SEEK_PRE_ROLL: Duration = Duration::from_millis(80);

/// [`SEEK_PRE_ROLL`] where `params` names Opus, and nothing for every other codec.
///
/// Per codec rather than for all of them: MP3, AAC and Vorbis hold state across packets too, but
/// widening their seeks is a behaviour change each has to earn on its own evidence.
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
    /// Reads all three off `extra_data`.
    ///
    /// Offsets rather than a parse: RFC 7845 section 5.1 froze the layout, and Symphonia has
    /// already validated the header this is a second read of. One too short to hold them is one
    /// stating none, which is what [`Default`] says — and family 0 is the right answer to default
    /// to, the two families building the same layout for the only counts family 0 admits.
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

/// Refuses a header stating a stream layout that is not the one `layout` was built as.
///
/// `opus_pure` derives the layout from the channel count and the family, and Symphonia's own
/// header reader stops at the family byte, so the stream count, the coupled count and the mapping
/// table reach nothing at all. A file disagreeing with the canonical layout has to be turned away
/// here or it decodes into the wrong channels with nothing anywhere to say so. Family 0 states
/// none of the three and has nothing to check.
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
        // Ogg admits a 19-byte identification packet and MP4 an 11-byte `dOps` body, both of which
        // stop one byte past the family, so a header missing its own mapping table gets this far.
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

/// The channel positions each count carries, in the order Opus hands them over.
///
/// RFC 7845 section 5.1.1.2's Vorbis orders, spelled in Symphonia's vocabulary so they can be
/// matched against the set its own header reader built from the same table. `Channels` is a set
/// and keeps no order, while a buffer's planes run in ascending channel position — which puts LFE
/// fourth where Opus puts it last.
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

/// Which Opus output channel feeds each of `channels`' planes, in the buffer's own plane order.
///
/// `None` where the container's set is not the one this count's Vorbis order names, which is also
/// what makes the index lookup below safe to trust: it counts the positions trailing the one asked
/// for and answers as readily for a position the set does not hold as for one it does.
fn plane_sources(channels: &Channels) -> Option<Box<[usize]>> {
    let order = *VORBIS_ORDER.get(channels.count().checked_sub(1)?)?;
    let named = order.iter().copied().fold(Position::empty(), |set, pos| set | pos);
    if *channels != Channels::Positioned(named) {
        return None;
    }

    let mut sources = vec![0; order.len()];
    for (channel, &pos) in order.iter().enumerate() {
        sources[channels.get_canonical_index_for_positioned_channel(pos)?] = channel;
    }
    Some(sources.into_boxed_slice())
}

/// The little-endian `u16` at `at`, or `None` where the slice ends before it.
fn le_u16(data: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

/// A frame count as a `usize`, saturating. The trim it feeds treats anything past the buffer as
/// the whole of it, so a count too large to represent empties the packet rather than wrapping.
fn trim_count(frames: u64) -> usize {
    usize::try_from(frames).unwrap_or(usize::MAX)
}

/// Opus over `opus_pure`, for the `CODEC_ID_OPUS` tracks Symphonia's own registry cannot serve.
pub struct OpusDecoder {
    params: AudioCodecParameters,
    decoder: opus_pure::OpusMSDecoder,
    buf: AudioBuffer<f32>,
    /// Interleaved scratch the codec writes into, sized once for the longest packet Opus allows so
    /// that nothing *here* grows. `opus_pure` still takes a couple of small allocations per packet
    /// for its own frame bookkeeping, bounded by the frame count and so per packet rather than per
    /// frame; closing that is an upstream change, not one this side can make.
    pcm: Vec<f32>,
    channels: usize,
    /// Which Opus output channel feeds each plane of [`Self::buf`], the two orders parting company
    /// from three channels up.
    sources: Box<[usize]>,
    /// Priming owed to the head of the stream, spent across as many packets as it takes: a stream
    /// cropped under RFC 7845 section 5.1 states at least 3840 frames, which outruns a 20 ms
    /// packet. [`AudioDecoder::reset`] does not re-arm it, so a seek cannot cut real audio.
    pre_skip: usize,
    /// Off, the priming stays in and [`super::file_decode`]'s timeline offset would describe
    /// frames still being handed over. `decode::make_decoder` takes the default, which is on.
    gapless: bool,
}

impl OpusDecoder {
    fn try_new(params: &AudioCodecParameters, opts: AudioDecoderOptions) -> SymphoniaResult<Self> {
        let Some(channels) = params.channels.clone() else {
            return unsupported_error("opus: channels or a channel layout is required");
        };
        let count = channels.count();
        let Some(sources) = plane_sources(&channels) else {
            return unsupported_error("opus: channel set is not one Opus states an order for");
        };

        let head = Head::read(params.extra_data.as_deref());
        // The multistream decoder for every count, not just above two: family 0 is a single stream
        // whose mapping is the identity, so the two decoders differ by a remux, and this way the
        // counts each family cannot carry are refused by `ChannelLayout::surround` rather than by
        // a range test here that would have to agree with it.
        let mut decoder =
            opus_pure::OpusMSDecoder::new(DECODE_RATE.cast_signed(), count, head.mapping_family)
                .map_err(|_| SymphoniaError::Unsupported("opus: stream cannot be decoded"))?;
        check_stated_layout(params.extra_data.as_deref(), decoder.layout())?;

        // RFC 7845 asks that this be applied by default, so it belongs here and not beside
        // ReplayGain, where the user's toggle would gate it. It describes the file rather than any
        // one stream, so every stream carries it.
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

        // De-interleave through the plane order rather than with `copy_from_slice_interleaved`,
        // which assumes the two agree. One path rather than a permuted one beside a direct one:
        // for the counts where they do agree this walks the same samples in the same order.
        let channels = self.channels;
        for (plane, &source) in self.buf.iter_planes_mut().zip(&self.sources) {
            for (sample, frame) in plane.iter_mut().zip(decoded.chunks_exact(channels)) {
                *sample = frame[source];
            }
        }

        if self.gapless {
            let start = trim_count(packet.trim_start.get());
            let end = trim_count(packet.trim_end.get());
            // What the priming has left to spend here. `AudioBuffer::trim` clears the buffer on an
            // overshoot and reports nothing back, so a pre-skip longer than one packet has to
            // carry its remainder or the rest of it plays.
            let available = frames.saturating_sub(start).saturating_sub(end);
            self.buf.trim(start.saturating_add(self.pre_skip), end);
            self.pre_skip = self.pre_skip.saturating_sub(available);
        }
        Ok(())
    }
}

impl AudioDecoder for OpusDecoder {
    fn reset(&mut self) {
        // Its only failure is the argument check `try_new` already passed, and one that refused
        // anyway would keep the state it has: an artefact after the seek, not silence.
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
