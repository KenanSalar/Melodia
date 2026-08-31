//! What both decoders need before and around the codec: the registry, the track pick, and the
//! packet cursor.
//!
//! [`super::file_decode`] and [`super::stream_decode`] differ in what they open — a file with a
//! length and a seek against a mount with neither — and in nothing after that. Resolving a codec
//! and handing out the samples it produces are the same questions for both, and the answers have
//! to agree or the split this module closed grows back one function at a time.
//!
//! **Why Symphonia 0.6 rather than the 0.5 rodio pins.** 0.5's probe picks a demuxer by matching a
//! two-byte marker and nothing else — its `format()` takes a `Hint` and discards it, and scoring is
//! still a `TODO` there. The only ADTS marker it registers is `0xFFF1`, so MPEG-2 ADTS (`0xFFF9`)
//! matches nothing at all, the search runs on into the payload, and the first stray `0xFFFB` in it
//! hands an AAC stream to the MP3 demuxer. That reader then hunts forever for two consecutive
//! similar frames it will never find: no decoder is built, no error is returned, and the audio
//! simply never starts. It was internet radio that hit it first, but nothing about the fault is
//! about the network — an MPEG-2 `.aac` file in a scanned folder is the same bytes and the same
//! silence. 0.6 scores each candidate against the frames that follow it and registers all four
//! ADTS sync words, and it answers for both paths so the same bytes cannot get two answers.

use std::sync::LazyLock;

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::{CODEC_ID_PCM_ALAW, CODEC_ID_PCM_MULAW};
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, CODEC_ID_NULL_AUDIO,
};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatReader, Track, TrackType};

use super::aac_config;
use super::audio::{ChannelCount, SampleRate};

/// Our own registry, rather than `symphonia::default::get_codecs`.
///
/// That one is fixed at whatever the crate's own features enable, so a decoder living outside it
/// can never be reached: Opus arrives as an adapter crate rather than a Symphonia feature, and
/// registering it is a line here and nothing at either call site.
static CODECS: LazyLock<CodecRegistry> = LazyLock::new(|| {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry
});

/// The track to decode, or `None` when the container declares nothing playable.
///
/// `default_track` matches the container's default flag before it looks at the codec
/// ([symphonia#258](https://github.com/pdeljanov/Symphonia/issues/258)), so a track naming a null
/// one can win the pick and the fallback that would have rejected it never runs.
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

/// Build a decoder for `params`, taking them by value because it rewrites an HE-AAC config on the
/// way past ([`super::aac_config`]).
///
/// The Symphonia error goes back unwrapped: what a caller says about it names a file or a station,
/// which is the half this cannot know.
pub(super) fn make_decoder(
    mut params: AudioCodecParameters,
) -> Result<Box<dyn AudioDecoder>, SymphoniaError> {
    aac_config::demote_he_aac(&mut params);
    drop_companded_sample_width(&mut params);
    CODECS.make_audio_decoder(&params, &AudioDecoderOptions::default())
}

/// Clears the sample widths on an A-law or mu-law track, which two Symphonia crates disagree about.
///
/// Both companded formats code 8 bits and decode to 16, and 0.6's PCM decoder reads the width
/// fields as the *decoded* one: it refuses anything but 16, defaulting to exactly that when they
/// are absent. `symphonia-format-caf` fills them from the file's own coded width instead, so every
/// A-law `.caf` fails to build a decoder at all. 0.5 had no such check and played them, which makes
/// this a file that stops playing rather than one that never did — the same shape of regression as
/// [`aac_config`], and the reason the fixture walk in `file_decode`'s tests opens every container
/// rather than trusting that a format the manifest enables is a format that decodes.
///
/// Clearing rather than correcting, because the decoder derives the right width from the codec and
/// a demuxer that already agreed loses nothing.
fn drop_companded_sample_width(params: &mut AudioCodecParameters) {
    if matches!(params.codec, CODEC_ID_PCM_ALAW | CODEC_ID_PCM_MULAW) {
        params.bits_per_sample = None;
        params.bits_per_coded_sample = None;
    }
}

/// What one decoded packet says the audio is.
struct Shape {
    channels: ChannelCount,
    sample_rate: SampleRate,
}

/// The packet being handed out, one interleaved sample at a time.
///
/// Both decoders own one and neither adds to it: the refill, the shape check rodio's fixed append
/// makes necessary, and the latch that stops a re-poll past the end serving the packet that ended
/// it are the same three answers whether the bytes came off a socket or off disk.
pub(super) struct Cursor {
    samples: Vec<f32>,
    next: usize,
    channels: ChannelCount,
    sample_rate: SampleRate,
    /// Set once there is nothing more to hand out.
    ended: bool,
}

impl Cursor {
    /// Decode enough to know the shape of what follows. `Ok(None)` where the source held no
    /// audio at all, against `Err` for one that could not be read.
    ///
    /// The two are worth telling apart here and nowhere else: this is the one call reached off the
    /// audio thread, and its answer becomes what the user is shown, where a permissions failure and
    /// an empty container read very differently.
    ///
    /// The first packet is decoded eagerly because the deck is told the channel count and sample
    /// rate when the source is appended, and rodio cannot renegotiate either afterwards.
    pub(super) fn open(
        format: &mut dyn FormatReader,
        decoder: &mut dyn AudioDecoder,
        track: u32,
    ) -> Result<Option<Self>, SymphoniaError> {
        let mut samples = Vec::new();
        let Some(shape) = fill(format, decoder, track, &mut samples)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            samples,
            next: 0,
            channels: shape.channels,
            sample_rate: shape.sample_rate,
            ended: false,
        }))
    }

    pub(super) fn channels(&self) -> ChannelCount {
        self.channels
    }

    pub(super) fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// Steps past the buffered packet, reporting the channel of the frame the puller was part way
    /// through so a seek can put it back there.
    pub(super) fn discard_buffered(&mut self) -> usize {
        let channel_phase = self.next % usize::from(self.channels.get());
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
            // A read that fails and a clean end are one thing from here: this runs on the audio
            // callback thread, where nothing may log and there is no channel back, so both reach
            // the caller as the only move it has.
            let Ok(Some(shape)) = fill(format, decoder, track, &mut self.samples) else {
                self.ended = true;
                return None;
            };
            // rodio was told the shape at the append and cannot renegotiate it, so a source that
            // changes one mid-track ends here rather than playing on at the wrong rate. A mount
            // doing it is a reconnect the feed thread then refuses.
            if shape.channels != self.channels || shape.sample_rate != self.sample_rate {
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

/// Decode packets into `samples` until one yields audio, or the source runs out.
///
/// A packet that fails to decode is skipped rather than ending it: a mount joined mid-frame opens
/// with a few that do, and a file with a damaged frame plays past it the way every other player
/// does. So is a packet that decodes to nothing, which is not the end of the stream either
/// ([symphonia#403](https://github.com/pdeljanov/Symphonia/issues/403)).
///
/// `Err(ResetRequired)` does end it, and that one is a choice rather than an oversight: Ogg raises
/// it where a chained stream starts a new physical one, and recovering means re-reading the track
/// list and rebuilding the decoder against parameters that may have moved. Neither reference player
/// does it. A mount gets the feed thread's reconnect on top; a chained file stops there.
///
/// A read that fails is kept apart from a clean end, which 0.6 made possible by spelling the end
/// as `Ok(None)` where 0.5 raised `UnexpectedEof`. Only [`Cursor::open`] has any use for the
/// distinction; [`Cursor::next_sample`] folds the two back together and says why.
fn fill(
    format: &mut dyn FormatReader,
    decoder: &mut dyn AudioDecoder,
    track: u32,
    samples: &mut Vec<f32>,
) -> Result<Option<Shape>, SymphoniaError> {
    loop {
        let Some(packet) = format.next_packet()? else {
            return Ok(None);
        };
        if packet.track_id != track {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = decoded.spec().clone();
                decoded.copy_to_vec_interleaved(samples);
                if !samples.is_empty() {
                    let channels =
                        u16::try_from(spec.channels().count()).ok().and_then(ChannelCount::new);
                    let (Some(channels), Some(sample_rate)) =
                        (channels, SampleRate::new(spec.rate()))
                    else {
                        // Nothing rodio can be told about, so it is the decoder rather than the
                        // read that this source runs out on.
                        return Err(SymphoniaError::Unsupported(
                            "channel count or sample rate out of range",
                        ));
                    };
                    return Ok(Some(Shape {
                        channels,
                        sample_rate,
                    }));
                }
            }
            Err(SymphoniaError::DecodeError(_)) => {}
            Err(e) => return Err(e),
        }
    }
}
