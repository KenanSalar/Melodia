//! Turning a file's bytes into samples: opening it, and the seek.
//!
//! What is this module's own is everything a file has and a mount does not — a length, a seek that
//! can land anywhere, and an end it reaches by itself. The probe, the codec registry and the packet
//! cursor are [`super::decode`]'s, shared with [`super::stream_decode`], and the argument for
//! decoding against Symphonia 0.6 rather than the 0.5 rodio pinned is in that module's `//!`.
//!
//! It replaced `rodio::Decoder`, so that type doubles as the specification: what must not be lost
//! is the frame-accurate seek, which is the one thing here neither reference implementation does.

use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSource;
use symphonia::core::units::{TimeBase, Timestamp};

use crate::error::AppError;

use super::audio::{AudioSource, ChannelCount, Sample, SampleRate, SeekError};
use super::decode;

/// How far short of a stated length a seek is allowed to land.
///
/// The stated length is not somewhere to land: some formats answer out of range and park the reader
/// at the end, and the rest land with nothing left to decode. Either way the deck drains and the
/// monitor reads it as the track finishing, and dragging the slider to its right edge asks for
/// exactly the length, so on the last queue entry that would end the queue.
///
/// The size is not derived from packet geometry — a packet here runs anywhere from an AAC frame to
/// a FLAC block, four times longer. It only has to clear the gap between the length a container
/// states and where its last decodable frame really is, which tags overstate by more than a frame
/// routinely, while staying short enough that a drag to the edge still sounds like the end.
const SEEK_END_MARGIN: Duration = Duration::from_millis(100);

/// A file as the demuxer wants it, with both of its answers taken once.
///
/// The mirror of [`super::stream_decode::LiveSource`], which answers no to both. A stated length is
/// what lets the probe reach trailing metadata and the seek land anywhere, neither of which a live
/// mount can offer. Symphonia ships its own `MediaSource` for `File` and re-reads the filesystem on
/// every call to say so, warning in its docs to cache what it returns; the demuxer asks once per
/// probe and again per seek.
///
/// No `BufReader` underneath: [`MediaSourceStream`] is already the read-ahead buffer, and a second
/// one only copies every byte again. Both reference players hand it the file directly.
struct FileSource {
    inner: File,
    seekable: bool,
    byte_len: Option<u64>,
}

impl FileSource {
    fn new(inner: File) -> Self {
        // Anything but a regular file is a pipe or a device wearing an audio extension: no length,
        // and nowhere to seek to.
        let regular = inner.metadata().ok().filter(Metadata::is_file);
        Self {
            seekable: regular.is_some(),
            byte_len: regular.map(|m| m.len()),
            inner,
        }
    }
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
        self.seekable
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
    cursor: decode::Cursor,
    time_base: Option<TimeBase>,
    total_duration: Option<Duration>,
}

impl FileDecoder {
    /// Probe `path`, build a decoder for its audio track, and decode enough of it to know the
    /// shape of what follows.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let file = File::open(path)
            .map_err(|e| AppError::Player(format!("Cannot open {}: {e}", path.display())))?;

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(extension);
        }

        let decode::Opened {
            format,
            decoder,
            track,
            cursor,
            time_base,
            total_duration,
        } = decode::open(Box::new(FileSource::new(file)), &hint)
            .map_err(|e| AppError::Player(format!("{} {e}", path.display())))?;

        Ok(Self {
            format,
            decoder,
            track,
            cursor,
            time_base,
            total_duration,
        })
    }

    /// Interleaved samples between where the demuxer landed and where the seek asked for, rounded
    /// up to a whole frame so the channels stay in step. Never under, so a seek cannot replay a
    /// frame the listener already heard; rodio rounded the other way and could.
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

        let rate = u128::from(self.cursor.shape().rate.get());
        let ticks = ahead * u128::from(time_base.numer.get()) * rate;
        let frames = ticks.div_ceil(u128::from(time_base.denom.get()));
        let Ok(frames) = usize::try_from(frames) else {
            return 0;
        };
        frames.saturating_mul(usize::from(self.cursor.shape().channels.get()))
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
        self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track)
    }
}

impl AudioSource for FileDecoder {
    #[inline]
    fn channels(&self) -> ChannelCount {
        self.cursor.shape().channels
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.cursor.shape().rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // `AudioSource::try_seek` promises to saturate wherever a length is known, and the caller
        // asks past the end routinely: the position it seeks to comes off the tags, which overshoot
        // the decoded length by a few frames often enough. The ceiling is short of the end rather
        // than on it, for [`SEEK_END_MARGIN`]'s reason.
        let pos =
            self.total_duration.map_or(pos, |total| pos.min(total.saturating_sub(SEEK_END_MARGIN)));

        let time = symphonia::core::units::Time::try_new(
            i64::try_from(pos.as_secs()).map_err(other)?,
            pos.subsec_nanos(),
        )
        .ok_or_else(|| other(AppError::Player("Seek position out of range".to_owned())))?;

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

        // A seek is a demuxer operation the decoder is told nothing about, so whatever overlap
        // state it holds now describes audio that is no longer adjacent. Unconditional because
        // that is what upstream asks for and what rodio did: the codecs keeping no state across
        // packets document their `reset` as doing nothing.
        self.decoder.reset();

        // Which channel of a frame the puller was part way through: a seek restarts on a frame
        // boundary, so without putting this back the next sample handed out is channel 0 where
        // channel 1 was due, and nothing downstream re-syncs. The deck's converter is never that
        // puller — it takes whole frames and seeks between them — but `try_seek` is on
        // `AudioSource`, so anything driving this iterator by hand can be.
        let channel_phase = self.cursor.discard_buffered();

        // A demuxer seek lands on a packet boundary, so without the trim every seek replays the
        // tail of what came before. Both reference players stop at the whole packet, and one says
        // in its own comment that it should not. rodio trimmed to the frame, and that is the
        // behaviour this path inherited and has to keep.
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
