//! What both decoders need around the codec: the registry, the track pick, the open and the
//! packet cursor. [`super::file_decode`] and [`super::stream_decode`] differ in what they hand
//! [`open`] and in nothing after, so the two can't drift apart.
//!
//! Against Symphonia 0.6, whose probe scores its candidates where 0.5's matched a two-byte marker
//! and handed MPEG-2 ADTS to the MP3 demuxer to hang
//! (`docs/adr/0007-symphonia-and-cpal-not-rodio.md`).

use std::sync::LazyLock;
use std::time::Duration;

use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ADPCM_IMA_QT, CODEC_ID_ADPCM_IMA_WAV, CODEC_ID_ADPCM_MS, CODEC_ID_MP1,
    CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_PCM_ALAW, CODEC_ID_PCM_MULAW,
    CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{
    AudioCodecId, AudioCodecParameters, AudioDecoder, AudioDecoderOptions, CODEC_ID_NULL_AUDIO,
};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, Track, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration as SymphoniaDuration, TimeBase};

use super::aac_config;
use super::audio::{ChannelCount, SampleRate, Shape, SourceFormat};

/// Ours rather than `symphonia::default::get_codecs`, which only the crate's own features fill, so
/// registering [`super::opus`] is one line here and nothing at either call site.
static CODECS: LazyLock<CodecRegistry> = LazyLock::new(|| {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry.register_audio_decoder::<super::opus::OpusDecoder>();
    registry
});

/// The track to decode, or `None` where nothing playable is declared. `default_track` honours the
/// container's default flag before the codec
/// ([symphonia#258](https://github.com/pdeljanov/Symphonia/issues/258)), so a null track can win
/// the pick unless filtered.
pub(super) fn audio_track(format: &dyn FormatReader) -> Option<&Track> {
    format
        .default_track(TrackType::Audio)
        .filter(|track| audio_params(track).is_some())
        .or_else(|| format.first_track_known_codec(TrackType::Audio))
}

/// The track's audio parameters, when it has some and they name a codec.
pub(super) fn audio_params(track: &Track) -> Option<&AudioCodecParameters> {
    track
        .codec_params
        .as_ref()
        .and_then(CodecParameters::audio)
        .filter(|params| params.codec != CODEC_ID_NULL_AUDIO)
}

/// Which way [`ticks_to_frames`] resolves a count the timebase does not divide evenly.
#[derive(Clone, Copy)]
pub(super) enum Rounding {
    /// Never claim more frames than the count is worth — a trim that overshoots cuts real audio.
    Down,
    /// Never claim fewer — a post-seek trim that undershoots replays the tail of the last packet.
    Up,
}

/// `u128` throughout, the product of a tick count, a numerator and a rate overflowing `u64` for
/// lengths a container can legitimately state. `None` where the result doesn't fit back.
pub(super) fn ticks_to_frames(
    ticks: u64,
    time_base: TimeBase,
    rate: SampleRate,
    rounding: Rounding,
) -> Option<u64> {
    let scaled = u128::from(ticks) * u128::from(time_base.numer.get()) * u128::from(rate.get());
    let denom = u128::from(time_base.denom.get());
    let frames = match rounding {
        Rounding::Down => scaled / denom,
        Rounding::Up => scaled.div_ceil(denom),
    };
    u64::try_from(frames).ok()
}

/// By value, to rewrite an HE-AAC config on the way past ([`super::aac_config`]). The error goes
/// back unwrapped, the caller being the one that can name the file or station.
pub(super) fn make_decoder(
    mut params: AudioCodecParameters,
) -> Result<Box<dyn AudioDecoder>, SymphoniaError> {
    aac_config::demote_he_aac(&mut params);
    drop_companded_sample_width(&mut params);
    CODECS.make_audio_decoder(&params, &AudioDecoderOptions::default())
}

/// Clears an A-law or mu-law track's sample widths, which 0.6's PCM decoder reads as the decoded 16
/// bits while `symphonia-format-caf` fills them with the coded 8, so every A-law `.caf` would fail
/// to build a decoder. Cleared rather than corrected, the decoder deriving it from the codec.
fn drop_companded_sample_width(params: &mut AudioCodecParameters) {
    if matches!(params.codec, CODEC_ID_PCM_ALAW | CODEC_ID_PCM_MULAW) {
        params.bits_per_sample = None;
        params.bits_per_coded_sample = None;
    }
}

/// Spelled out, Symphonia carrying no such flag. The companded PCMs count, coding a 13- or 14-bit
/// signal in 8. A lossy codec added to [`CODECS`] belongs here too, or its tracks read as lossless.
fn lossy_codec(codec: AudioCodecId) -> bool {
    matches!(
        codec,
        CODEC_ID_MP1
            | CODEC_ID_MP2
            | CODEC_ID_MP3
            | CODEC_ID_AAC
            | CODEC_ID_VORBIS
            | CODEC_ID_OPUS
            | CODEC_ID_ADPCM_MS
            | CODEC_ID_ADPCM_IMA_WAV
            | CODEC_ID_ADPCM_IMA_QT
            | CODEC_ID_PCM_ALAW
            | CODEC_ID_PCM_MULAW
    )
}

/// A demuxer, a decoder for its audio track, and enough of that track decoded to know its shape.
/// `time_base` and `total_duration` are the two a mount drops.
pub(super) struct Opened {
    pub format: Box<dyn FormatReader>,
    pub decoder: Box<dyn AudioDecoder>,
    pub track: u32,
    pub cursor: Cursor,
    pub source_format: SourceFormat,
    pub time_base: Option<TimeBase>,
    pub total_duration: Option<Duration>,
}

/// Phrased to follow the source's name, so one message reads "`/music/x.flac`: has no audio" or
/// "The station's stream: has no audio".
#[derive(Debug, thiserror::Error)]
pub(super) enum OpenError {
    #[error("could not be read: {0}")]
    Unreadable(#[source] SymphoniaError),
    #[error("has no audio")]
    NoAudio,
    #[error("names no audio codec")]
    NoCodec,
    #[error("could not be decoded: {0}")]
    Undecodable(#[source] SymphoniaError),
    #[error("decoded to nothing")]
    Empty,
}

/// `hint` is a file's extension or a mount's content type, which 0.6 scores rather than trusts.
pub(super) fn open(source: Box<dyn MediaSource>, hint: &Hint) -> Result<Opened, OpenError> {
    let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
    let mut format = symphonia::default::get_probe()
        .probe(hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(OpenError::Unreadable)?;

    let track = audio_track(&*format).ok_or(OpenError::NoAudio)?;
    let params = audio_params(track).ok_or(OpenError::NoCodec)?.clone();
    let stated_bits = params.bits_per_sample;
    let lossy = lossy_codec(params.codec);
    let track_id = track.id;
    let time_base = track.time_base;
    let total_duration = playing_time(&*format, track);

    let mut decoder = make_decoder(params).map_err(OpenError::Undecodable)?;
    let (cursor, decoded) = Cursor::open(&mut *format, &mut *decoder, track_id)
        .map_err(OpenError::Unreadable)?
        .ok_or(OpenError::Empty)?;
    let source_format = SourceFormat { lossy, ..narrowed_to_stated(decoded, stated_bits) };

    Ok(Opened {
        format,
        decoder,
        track: track_id,
        cursor,
        source_format,
        time_base,
        total_duration,
    })
}

/// A decoder hands integer PCM over in whatever buffer it has, so a 24-bit file can arrive in 32,
/// and read at that width it would count as losing low bits on the way to `f32`.
fn narrowed_to_stated(decoded: SourceFormat, stated_bits: Option<u32>) -> SourceFormat {
    let stated = stated_bits.and_then(|bits| u8::try_from(bits).ok());
    match stated {
        Some(bits) if !decoded.float && bits < decoded.bits => SourceFormat { bits, ..decoded },
        _ => decoded,
    }
}

/// The width and kind of sample a decoder produced, before the copy flattens it to `f32`.
fn decoded_format(decoded: &GenericAudioBufferRef<'_>) -> SourceFormat {
    let (bits, float) = match decoded {
        GenericAudioBufferRef::U8(_) | GenericAudioBufferRef::S8(_) => (8, false),
        GenericAudioBufferRef::U16(_) | GenericAudioBufferRef::S16(_) => (16, false),
        GenericAudioBufferRef::U24(_) | GenericAudioBufferRef::S24(_) => (24, false),
        GenericAudioBufferRef::U32(_) | GenericAudioBufferRef::S32(_) => (32, false),
        GenericAudioBufferRef::F32(_) => (32, true),
        GenericAudioBufferRef::F64(_) => (64, true),
    };
    SourceFormat { bits, float, lossy: false }
}

/// Three sources, none answering for every container: `Track::duration`, `num_frames`, and the
/// segment duration Matroska puts on the media instead. A stated zero is the reader not knowing (a
/// fragmented MP4 zeroes both track fields), so each is discarded before the next is tried.
fn playing_time(format: &dyn FormatReader, track: &Track) -> Option<Duration> {
    let from_track = track.time_base.and_then(|time_base| {
        let frames = || track.num_frames.filter(|frames| *frames > 0).map(SymphoniaDuration::new);
        let ticks = track.duration.filter(|stated| stated.get() > 0).or_else(frames)?;
        time_base.calc_duration(ticks)
    });
    let from_media = || {
        let media = format.media_info();
        media.time_base?.calc_duration(media.duration.filter(|stated| stated.get() > 0)?)
    };

    let (seconds, nanos) = from_track.or_else(from_media)?.parts();
    Some(Duration::new(u64::try_from(seconds).ok()?, nanos))
}

/// The packet being handed out. Both decoders own one and add nothing, so the refill, the shape
/// check and the end latch can't drift between them.
pub(super) struct Cursor {
    samples: Vec<f32>,
    next: usize,
    shape: Shape,
    /// Set once there is nothing more to hand out.
    ended: bool,
}

impl Cursor {
    /// Decodes the first packet eagerly: the deck builds its converter from the shape at the
    /// append, and the decoder's sample format can be read only here. `Ok(None)` is a source with
    /// no audio and `Err` an unreadable one, which the user is shown differently.
    pub(super) fn open(
        format: &mut dyn FormatReader,
        decoder: &mut dyn AudioDecoder,
        track: u32,
    ) -> Result<Option<(Self, SourceFormat)>, SymphoniaError> {
        let mut samples = Vec::new();
        let Some(packet) = fill(format, decoder, track, &mut samples)? else {
            return Ok(None);
        };
        let cursor = Self { samples, next: 0, shape: packet.shape, ended: false };
        Ok(Some((cursor, packet.format)))
    }

    pub(super) fn shape(&self) -> Shape {
        self.shape
    }

    /// Steps past the buffered packet, reporting the channel of the frame the puller was part way
    /// through so a seek can put it back there.
    pub(super) fn discard_buffered(&mut self) -> usize {
        let channel_phase = self.next % usize::from(self.shape.channels.get());
        self.next = self.samples.len();
        self.ended = false;
        channel_phase
    }

    pub(super) fn next_sample(
        &mut self,
        format: &mut dyn FormatReader,
        decoder: &mut dyn AudioDecoder,
        track: u32,
    ) -> Option<f32> {
        if self.ended {
            return None;
        }
        if self.next >= self.samples.len() {
            // A failed read and a clean end are one here: on the audio callback nothing may log,
            // and ending is the only move the caller has.
            let Ok(Some(packet)) = fill(format, decoder, track, &mut self.samples) else {
                self.ended = true;
                return None;
            };
            // The deck's converter is fixed to the shape at the append, so a mid-track change ends
            // here; on a mount it is a reconnect the feed thread then refuses.
            if packet.shape != self.shape {
                self.ended = true;
                return None;
            }
            self.next = 0;
        }
        let sample = *self.samples.get(self.next)?;
        self.next += 1;
        Some(sample)
    }
}

/// Decodes packets into `samples` until one yields audio, or the source runs out. A packet that
/// fails to decode, or decodes to nothing
/// ([symphonia#403](https://github.com/pdeljanov/Symphonia/issues/403)), is skipped. A
/// `ResetRequired` does end it: recovering a chained Ogg means rebuilding the decoder against
/// parameters that may have moved. A failed read stays apart from a clean end for [`Cursor::open`].
fn fill(
    format: &mut dyn FormatReader,
    decoder: &mut dyn AudioDecoder,
    track: u32,
    samples: &mut Vec<f32>,
) -> Result<Option<Packet>, SymphoniaError> {
    loop {
        let Some(packet) = format.next_packet()? else {
            return Ok(None);
        };
        if packet.track_id != track {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                // Borrowed, not cloned: `Channels` has a boxed variant, so a clone would be an
                // allocation per packet on the audio callback, where a local file decodes.
                let spec = decoded.spec();
                decoded.copy_to_vec_interleaved(samples);
                if !samples.is_empty() {
                    let channels =
                        u16::try_from(spec.channels().count()).ok().and_then(ChannelCount::new);
                    let (Some(channels), Some(rate)) = (channels, SampleRate::new(spec.rate()))
                    else {
                        // Nothing a deck can be handed, so it is the decoder this runs out on.
                        return Err(SymphoniaError::Unsupported(
                            "channel count or sample rate out of range",
                        ));
                    };
                    let shape = Shape { channels, rate };
                    return Ok(Some(Packet { shape, format: decoded_format(&decoded) }));
                }
            }
            Err(SymphoniaError::DecodeError(_)) => {}
            Err(e) => return Err(e),
        }
    }
}

/// What [`fill`] decoded, as far as anything above it asks.
struct Packet {
    shape: Shape,
    format: SourceFormat,
}

#[cfg(test)]
#[path = "tests/decode_tests.rs"]
mod tests;
