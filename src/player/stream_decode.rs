//! Turning a live mount's bytes into samples: the demuxer, the codec, and the packet loop.
//!
//! **Why this is not `rodio::Decoder`.** Rodio decodes through Symphonia 0.5, whose probe picks a
//! demuxer by matching a two-byte marker and nothing else — its `format()` takes a `Hint` and
//! discards it, and scoring is still a `TODO` there. The only ADTS marker 0.5 registers is
//! `0xFFF1`, so a station sending MPEG-2 ADTS (`0xFFF9`) matches nothing at all, the search runs on
//! into the payload, and the first stray `0xFFFB` in it hands an AAC stream to the MP3 demuxer.
//! That reader then hunts forever for two consecutive similar frames it will never find: no
//! decoder is built, no error is returned, and the station simply never starts. Symphonia 0.6
//! scores each candidate against the frames that follow it and registers all four ADTS sync words,
//! which is why the stream path is decoded against that version while local files stay on rodio's.
//!
//! The two majors never meet. [`super::prebuffer`]'s ring is the seam: what reaches rodio is
//! `f32`, not a decoder.

use std::io::{Read, Seek, SeekFrom};

use rodio::{ChannelCount, Sample, SampleRate};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions, CODEC_ID_NULL_AUDIO};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, Track, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::error::AppError;

/// A reader the demuxer may read but must never seek or measure.
///
/// Both answers are the shape of a live mount rather than a shortcoming: there is no end to seek
/// to, and a stated length is what sends the probe hunting for trailing metadata that will never
/// arrive. The `Seek` impl exists only because [`MediaSource`] requires it.
pub struct LiveSource<R>(pub R);

impl<R: Read> Read for LiveSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl<R: Seek> Seek for LiveSource<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for LiveSource<R> {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// A live stream's demuxer and codec, handing out interleaved samples one at a time.
///
/// The iterator ends when the mount does, or when it stops being the shape the deck was told
/// about, both of which the feed thread reads as "try reconnecting". A packet that fails to decode
/// is skipped rather than ending it: on a mount joined mid-frame the first few routinely do.
pub struct StreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    /// The packet currently being handed out, interleaved.
    samples: Vec<f32>,
    next: usize,
    channels: ChannelCount,
    sample_rate: SampleRate,
    /// Set once there is nothing more to hand out, so a re-poll past the end cannot serve the
    /// packet that ended it.
    ended: bool,
}

impl StreamDecoder {
    /// Probe `source`, build a decoder for its audio track, and decode enough of it to know the
    /// shape of what follows.
    ///
    /// The first packet is decoded here rather than lazily because the deck is told the channel
    /// count and sample rate when the source is appended, and rodio cannot renegotiate either
    /// afterwards.
    pub fn open(source: Box<dyn MediaSource>, mime: Option<&str>) -> Result<Self, AppError> {
        let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());

        // Passed for the same reason the content type is read at all, though 0.6 still resolves
        // the format by scoring rather than by hint.
        let mut hint = Hint::new();
        if let Some(mime) = mime {
            hint.mime_type(mime);
        }

        let mut format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| AppError::Player(format!("Cannot read the station's stream: {e}")))?;

        let track = format
            .default_track(TrackType::Audio)
            .filter(|track| names_a_codec(track))
            .or_else(|| format.first_track_known_codec(TrackType::Audio))
            .ok_or_else(|| AppError::Player("The station's stream has no audio".to_owned()))?;
        let track_id = track.id;
        let params = track
            .codec_params
            .as_ref()
            .and_then(CodecParameters::audio)
            .ok_or_else(|| {
                AppError::Player("The station's stream names no audio codec".to_owned())
            })?
            .clone();

        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| AppError::Player(format!("Cannot decode the station's stream: {e}")))?;

        let mut samples = Vec::new();
        let shape = fill(&mut *format, &mut *decoder, track_id, &mut samples).ok_or_else(|| {
            AppError::Player("The station's stream ended before it played".to_owned())
        })?;

        Ok(Self {
            format,
            decoder,
            track: track_id,
            samples,
            next: 0,
            channels: shape.channels,
            sample_rate: shape.sample_rate,
            ended: false,
        })
    }

    pub fn channels(&self) -> ChannelCount {
        self.channels
    }

    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }
}

impl Iterator for StreamDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        if self.next >= self.samples.len() {
            let Some(shape) =
                fill(&mut *self.format, &mut *self.decoder, self.track, &mut self.samples)
            else {
                self.ended = true;
                return None;
            };
            // rodio cannot renegotiate the shape it was told at the append, so a mount that
            // changes one mid-connection ends here rather than playing on at the wrong rate: the
            // feed thread reads that as a reconnect, which refuses the new format and stops.
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

/// Whether `track` names an audio codec, rather than declaring a track nothing can decode.
///
/// `default_track` matches the container's default flag before it looks at the codec
/// ([symphonia#258](https://github.com/pdeljanov/Symphonia/issues/258)), so a null one wins the
/// pick and the fallback that would have rejected it never runs.
fn names_a_codec(track: &Track) -> bool {
    track
        .codec_params
        .as_ref()
        .and_then(CodecParameters::audio)
        .is_some_and(|params| params.codec != CODEC_ID_NULL_AUDIO)
}

/// What one decoded packet says the audio is.
struct Shape {
    channels: ChannelCount,
    sample_rate: SampleRate,
}

/// Decode packets into `samples` until one yields audio, or the mount stops.
///
/// `None` covers every way a live stream can stop being one — the server closing the connection,
/// the socket dropping, a codec the container named but nothing can decode — because the feed
/// thread's answer to all of them is the same: reconnect, or give up once its budget is spent.
fn fill(
    format: &mut dyn FormatReader,
    decoder: &mut dyn AudioDecoder,
    track: u32,
    samples: &mut Vec<f32>,
) -> Option<Shape> {
    loop {
        let Ok(Some(packet)) = format.next_packet() else {
            return None;
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
                        u16::try_from(spec.channels().count()).ok().and_then(ChannelCount::new)?;
                    return Some(Shape {
                        channels,
                        sample_rate: SampleRate::new(spec.rate())?,
                    });
                }
            }
            // A mount joined mid-frame starts with a partial one, and a live stream drops packets
            // on its own account; neither is the end of the station.
            Err(SymphoniaError::DecodeError(_)) => {}
            Err(_) => return None,
        }
    }
}
