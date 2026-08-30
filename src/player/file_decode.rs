//! Turning a file's bytes into samples: opening it, the packet loop and the seek.
//!
//! What is this module's own is everything a file has and a mount does not — a length, a seek that
//! can land anywhere, and an end it reaches by itself. The probe, the codec registry and the packet
//! loop are [`super::decode`]'s, shared with [`super::stream_decode`], and the argument for
//! decoding against Symphonia 0.6 rather than the 0.5 rodio pins is in that module's `//!`.
//!
//! It replaces `rodio::Decoder`, so that type doubles as the specification: what must not be lost
//! is the frame-accurate seek, which is the one thing here neither reference implementation does.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::codecs::audio::well_known::CODEC_ID_MP3;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, Track};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration as SymphoniaDuration, TimeBase, Timestamp};

use crate::error::AppError;

use super::decode;

/// Symphonia pulls frames in chunks well above the std 8 KB default for most formats, so a small
/// buffer costs a refill per frame. This covers typical FLAC and MP3 frame clusters without
/// meaningful per-track memory.
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// A file as the demuxer wants it: seekable, and with a length it can trust.
///
/// The mirror of [`super::stream_decode::LiveSource`], which answers no to both. A stated length is
/// what lets the probe reach trailing metadata and the seek land anywhere, neither of which a live
/// mount can offer.
struct FileSource {
    inner: BufReader<File>,
    byte_len: Option<u64>,
}

impl Read for FileSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for FileSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl MediaSource for FileSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.byte_len
    }
}

/// A file's demuxer and codec, handing out interleaved samples one at a time.
pub struct FileDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    /// The packet currently being handed out, interleaved.
    samples: Vec<f32>,
    next: usize,
    channels: ChannelCount,
    sample_rate: SampleRate,
    time_base: Option<TimeBase>,
    total_duration: Option<Duration>,
    /// Whether this codec is one of the few a seek leaves in a state only a reset clears.
    reset_after_seek: bool,
    /// Set once there is nothing more to hand out, so a re-poll past the end cannot serve the
    /// packet that ended it.
    ended: bool,
}

impl FileDecoder {
    /// Probe `path`, build a decoder for its audio track, and decode enough of it to know the
    /// shape of what follows.
    ///
    /// The first packet is decoded here rather than lazily for the reason the stream path does it:
    /// the deck is told the channel count and sample rate when the source is appended, and rodio
    /// cannot renegotiate either afterwards.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let file = File::open(path)
            .map_err(|e| AppError::Player(format!("Cannot open {}: {e}", path.display())))?;
        let byte_len = file.metadata().map(|m| m.len()).ok();
        let source = FileSource {
            inner: BufReader::with_capacity(READ_BUFFER_BYTES, file),
            byte_len,
        };
        let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(extension);
        }

        let mut format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| AppError::Player(format!("Cannot read {}: {e}", path.display())))?;

        let track = decode::audio_track(&*format)
            .ok_or_else(|| AppError::Player(format!("{} has no audio", path.display())))?;
        let params = decode::audio_params(track)
            .ok_or_else(|| AppError::Player(format!("{} names no audio codec", path.display())))?
            .clone();
        let reset_after_seek = params.codec == CODEC_ID_MP3;
        let track_id = track.id;
        let time_base = track.time_base;
        let total_duration = playing_time(&*format, track);

        let mut decoder = decode::make_decoder(params)
            .map_err(|e| AppError::Player(format!("Decode error for {}: {e}", path.display())))?;

        let mut samples = Vec::new();
        let shape = decode::fill(&mut *format, &mut *decoder, track_id, &mut samples)
            .ok_or_else(|| AppError::Player(format!("{} decoded to nothing", path.display())))?;

        Ok(Self {
            format,
            decoder,
            track: track_id,
            samples,
            next: 0,
            channels: shape.channels,
            sample_rate: shape.sample_rate,
            time_base,
            total_duration,
            reset_after_seek,
            ended: false,
        })
    }

    /// Interleaved samples between where the demuxer landed and where the seek asked for, rounded
    /// up to a whole frame so the channels stay in step. Never under, so a seek cannot replay a
    /// frame the listener already heard; rodio rounds the other way and can.
    ///
    /// Integer arithmetic against the timebase's own ratio rather than seconds, so a rate the
    /// timebase does not divide evenly cannot drift the answer by a frame.
    fn samples_before(&self, required: Timestamp, actual: Timestamp) -> usize {
        let Some(time_base) = self.time_base else {
            return 0;
        };
        let ahead = required.get().saturating_sub(actual.get());
        let Ok(ahead) = u128::try_from(ahead) else {
            return 0;
        };

        let ticks = ahead * u128::from(time_base.numer.get()) * u128::from(self.sample_rate.get());
        let frames = ticks.div_ceil(u128::from(time_base.denom.get()));
        let Ok(frames) = usize::try_from(frames) else {
            return 0;
        };
        frames.saturating_mul(usize::from(self.channels.get()))
    }

    /// Discard `count` interleaved samples, decoding as far as it takes.
    fn skip(&mut self, count: usize) {
        for _ in 0..count {
            if self.next().is_none() {
                return;
            }
        }
    }
}

/// How long the track plays for.
///
/// Three sources, because no one of them answers for every container. `Track::duration` is the one
/// upstream says to present, a timebase not having to be the reciprocal of the frame rate;
/// `num_frames` is where a container states its length in frames instead; and Matroska states
/// neither on the track, putting the segment's own duration on the media. A `.mka` reaching the
/// library reading 0:00 is what this exists to prevent, and it is the container that needs the
/// third.
///
/// A stated zero is the reader saying it doesn't know rather than the file being empty — a
/// fragmented MP4 zeroes both track fields — so each one has to be discarded before the next is
/// reached, or the first answers for all three.
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

/// How long the file at `path` plays for, or `None` when it names no duration or no decoder is
/// registered for its codec.
///
/// The scan path's answer of last resort. Lofty reads duration off the same parse that reads the
/// tags, so a file it can't identify (a Matroska or CAF one, say) reaches the database with no
/// length at all unless someone asks the decoder instead (`media::metadata`). It costs a probe plus
/// one decoded packet, which is why it stays on that failure path rather than running for every
/// file scanned.
pub fn probe_duration(path: &Path) -> Option<Duration> {
    FileDecoder::open(path).ok()?.total_duration()
}

impl Iterator for FileDecoder {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        if self.next >= self.samples.len() {
            let Some(shape) =
                decode::fill(&mut *self.format, &mut *self.decoder, self.track, &mut self.samples)
            else {
                self.ended = true;
                return None;
            };
            // rodio was told the shape at the append and cannot renegotiate it, so a file that
            // changes one mid-track ends here rather than playing on at the wrong rate.
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

impl Source for FileDecoder {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        // Never `None`: that reaches `UniformSourceIterator::bootstrap` as an unbounded `Take`, so
        // the mixer builds one `SampleRateConverter` out of whichever source reached the deck first
        // and never gets a boundary to rebuild it at. The packet is the boundary rodio's own
        // decoder hands up, and it names the whole one rather than what is left of it for the same
        // reason: a span of zero would have the converter rebuild against an empty `Take`.
        Some(self.samples.len())
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.channels
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // `Source::try_seek` promises to saturate wherever a length is known, and the caller asks
        // past the end routinely: the position it seeks to comes off the tags, which overshoot the
        // decoded length by a few frames often enough. Unclamped, the demuxer answers out of range
        // or parks at the end, and a deck draining reads to the monitor as the track finishing.
        let pos = self.total_duration.map_or(pos, |total| pos.min(total));

        let time = symphonia::core::units::Time::try_new(
            i64::try_from(pos.as_secs()).map_err(other)?,
            pos.subsec_nanos(),
        )
        .ok_or_else(|| other(AppError::Player("Seek position out of range".to_owned())))?;

        // Which channel of a frame the puller is part way through. A seek restarts on a frame
        // boundary, so without putting this back the next sample handed out is channel 0 where
        // channel 1 was due, and rodio's channel converter — which keeps its own phase and is not
        // reset by a seek — runs one sample out of step for the rest of the track. That is the
        // permanent channel swap, and rodio's own decoder carries these two lines for it.
        let channel_phase = self.next % usize::from(self.channels.get());

        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track),
                },
            )
            .map_err(other)?;

        // The buffer is left alone and only stepped past, so `current_span_len` keeps naming a
        // real packet until the next pull replaces it.
        self.next = self.samples.len();
        self.ended = false;

        // Resetting is not universally safe — it sends some containers back to the start
        // ([symphonia#274](https://github.com/pdeljanov/Symphonia/issues/274)) — so it happens for
        // the one codec that needs it rather than for all of them.
        if self.reset_after_seek {
            self.decoder.reset();
        }

        // A demuxer seek lands on a packet boundary, so without the trim every seek replays the
        // tail of what came before. Both reference players stop at the whole packet, and one says
        // in its own comment that it should not; rodio trims to the frame today, and that is the
        // behaviour this path has to keep.
        let trim = self.samples_before(seeked.required_ts, seeked.actual_ts);
        self.skip(trim + channel_phase);
        Ok(())
    }
}

fn other(source: impl std::error::Error + Send + Sync + 'static) -> SeekError {
    SeekError::Other(Arc::new(source))
}

#[cfg(test)]
#[path = "tests/file_decode_tests.rs"]
mod tests;
