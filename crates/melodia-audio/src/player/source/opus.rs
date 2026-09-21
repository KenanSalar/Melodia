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
//! **The priming comes off here rather than in [`super::file_decode`], and that costs a known
//! skew.** The Ogg mapper leaves `absgp_to_ts` at identity, so the granule position arrives
//! still carrying the pre-skip and the first packet's timestamp is zero rather than negative: a
//! stated duration runs that much long and a seek lands that much early, 6.5 ms at the 312
//! frames RFC 7845 recommends. What it buys is the other two containers, neither of which
//! stands a `Track::delay` up for a reader above the codec to find. That field is where
//! exactness would come from if the skew ever starts to matter.
//!
//! **No soft clip.** The float output rings slightly past plus or minus one, as libopus does,
//! and the DSP chain clamps whenever anything is enabled. A nonlinearity on every Opus track,
//! to correct an overshoot the device converts identically either way, is the wrong trade.

use opus_pure::MAX_PACKET_SAMPLES;
use symphonia::core::audio::{
    AsGenericAudioBufferRef, AudioBuffer, AudioMut, AudioSpec, GenericAudioBufferRef,
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

/// What the identification packet says that reaches this decoder and nowhere else.
#[derive(Clone, Copy, Default)]
struct Head {
    /// Frames of encoder priming at 48 kHz, ahead of the first real sample.
    pre_skip: u16,
    /// Playback gain in Q7.8 dB, which RFC 7845 section 5.1 asks be applied by default.
    gain_q8: i16,
}

impl Head {
    /// Reads both off `extra_data`.
    ///
    /// Offsets rather than a parse: RFC 7845 section 5.1 froze the layout, and Symphonia has
    /// already validated the header this is a second read of. One too short to hold them is one
    /// stating neither, which is what [`Default`] says.
    fn read(extra_data: Option<&[u8]>) -> Self {
        /// From the `OpusHead` magic, which every container keeps at the front of `extra_data`.
        const PRE_SKIP: usize = 10;
        const OUTPUT_GAIN: usize = 16;

        let Some(data) = extra_data else {
            return Self::default();
        };
        let (Some(pre_skip), Some(gain)) = (le_u16(data, PRE_SKIP), le_u16(data, OUTPUT_GAIN))
        else {
            return Self::default();
        };
        Self { pre_skip, gain_q8: gain.cast_signed() }
    }
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
    decoder: opus_pure::OpusDecoder,
    buf: AudioBuffer<f32>,
    /// Interleaved scratch the codec writes into, sized once for the longest packet Opus allows
    /// so that nothing on the decode path grows.
    pcm: Vec<f32>,
    channels: usize,
    /// Priming owed to the head of the stream, taken off with the first packet's own trim and
    /// spent there. [`AudioDecoder::reset`] does not re-arm it, so a seek cannot cut real audio.
    pre_skip: u32,
    gapless: bool,
}

impl OpusDecoder {
    fn try_new(params: &AudioCodecParameters, opts: AudioDecoderOptions) -> SymphoniaResult<Self> {
        let Some(channels) = params.channels.clone() else {
            return unsupported_error("opus: channels or a channel layout is required");
        };
        let count = channels.count();

        // Surround is a multistream decode plus a plane reorder that `Channels` cannot express,
        // being a set of positions rather than an order. Symphonia's own header parse accepts
        // mapping family 1 up to eight channels, so such a file reaches here rather than
        // stopping above it.
        if !(1..=2).contains(&count) {
            return unsupported_error("opus: only mono and stereo are decoded");
        }

        let head = Head::read(params.extra_data.as_deref());
        let mut decoder = opus_pure::OpusDecoder::new(DECODE_RATE.cast_signed(), count)
            .map_err(|_| SymphoniaError::Unsupported("opus: stream cannot be decoded"))?;

        // RFC 7845 asks that this be applied by default, so it belongs here and not beside
        // ReplayGain, where the user's toggle would gate it.
        decoder.gain_q8 = i32::from(head.gain_q8);

        Ok(Self {
            params: params.clone(),
            decoder,
            buf: AudioBuffer::new(AudioSpec::new(DECODE_RATE, channels), MAX_PACKET_SAMPLES),
            pcm: vec![0.0; MAX_PACKET_SAMPLES * count],
            channels: count,
            pre_skip: u32::from(head.pre_skip),
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
        self.buf.copy_from_slice_interleaved(&decoded);

        if self.gapless {
            let head = packet.trim_start.get().saturating_add(u64::from(self.pre_skip));
            self.buf.trim(trim_count(head), trim_count(packet.trim_end.get()));
            self.pre_skip = 0;
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
