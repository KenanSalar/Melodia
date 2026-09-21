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
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSource;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use melodia_core::error::AppError;

use super::aac_trim::{self, Trim};
use super::audio::{
    AudioSource, ChannelCount, Sample, SampleRate, SeekError, frames_in, frames_to_duration,
    interleaved,
};
use super::decode::{self, Rounding};
use super::opus;

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
        Self { seekable: regular.is_some(), byte_len: regular.map(|m| m.len()), inner }
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
    /// The encoder padding this file states, for the seek that has to add its head back on.
    trim: Option<Trim>,
    /// Interleaved samples of real audio left ahead of that padding.
    remaining: Option<u64>,
}

impl FileDecoder {
    /// Probe `path`, build a decoder for its audio track, and decode enough of it to know the
    /// shape of what follows.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let mut file = File::open(path)
            .map_err(|e| AppError::Player(format!("Cannot open {}: {e}", path.display())))?;

        // Read while the handle is still ours: the edit list is the half of an AAC file's encoder
        // padding that the demuxer parses and keeps to itself, and it is the same open either way.
        let edits = aac_trim::edit_lists(&mut file);

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(extension);
        }

        let decode::Opened { format, decoder, track, cursor, time_base, total_duration } =
            decode::open(Box::new(FileSource::new(file)), &hint)
                .map_err(|e| AppError::Player(format!("{} {e}", path.display())))?;

        let mut decoded = Self {
            format,
            decoder,
            track,
            cursor,
            time_base,
            total_duration,
            trim: None,
            remaining: None,
        };
        decoded.install_trim(&edits);
        Ok(decoded)
    }

    /// Lines this source's timeline up with the demuxer's, dropping the priming where nothing
    /// below has.
    ///
    /// Two codecs state padding and they arrive here in opposite states. AAC states it where no
    /// decoder reads it, so the head comes off through [`Self::skip`] rather than a window inside
    /// the cursor: the cursor is shared with the stream decoder, and a live mount has neither a
    /// container header stating a delay nor a gapless transition to spoil. Opus states it in the
    /// identification packet and [`super::opus`] has already taken it off, so all that is left is
    /// the offset it leaves behind on a timeline that still counts those frames. Every other codec
    /// leaves here untouched.
    ///
    /// Runs on the thread that opened the file, which is never the audio callback: all three call
    /// sites hoist the open off the deck lock for the position monitor's sake.
    fn install_trim(&mut self, edits: &[aac_trim::Edit]) {
        if let Some(trim) = self.aac_padding(edits) {
            let shape = self.cursor.shape();
            self.trim = Some(trim);
            // Ahead of the counts, which describe the audio left once this is gone.
            self.skip(
                usize::try_from(interleaved(trim.head, shape.channels)).unwrap_or(usize::MAX),
            );
            self.measure_playable();
            log::debug!(
                "AAC encoder padding: {} priming frames dropped, {:?} of audio",
                trim.head,
                self.total_duration
            );
            return;
        }

        if let Some(head) = self.opus_priming() {
            self.trim = Some(Trim { head, playable: None });
            self.measure_playable();
            log::debug!(
                "Opus pre-skip: {head} frames already off the samples, {:?} of audio",
                self.total_duration
            );
        }
    }

    /// What an AAC file states about its own encoder padding, in decoded frames.
    fn aac_padding(&mut self, edits: &[aac_trim::Edit]) -> Option<Trim> {
        let timing = decode::audio_track(&*self.format).and_then(aac_trim::aac_timing)?;
        aac_trim::resolve(&timing, &self.format.metadata(), edits, self.cursor.shape().rate)
    }

    /// The priming [`super::opus`] has already dropped, which the demuxer's timeline still counts.
    ///
    /// Gated on the codec rather than on the field being filled: the MP3 and CAF readers fill
    /// `Track::delay` too, and MP3's decoder acts on its own packet trims already, so a blanket
    /// rule would take the same delay off twice. Ogg is the only container that fills it for Opus,
    /// and the only one that needs to: Matroska subtracts its `CodecDelay` from the timestamps
    /// instead, so its timeline already agrees with the samples, and MP4 fills nothing.
    fn opus_priming(&self) -> Option<u64> {
        let track = decode::audio_track(&*self.format)?;
        if decode::audio_params(track)?.codec != CODEC_ID_OPUS {
            return None;
        }
        track.delay.map(u64::from)
    }

    /// Restates the length, and arms the end count, against the real audio rather than against
    /// everything the container holds.
    ///
    /// A file stating a head and no length still plays for that much less than the container says,
    /// and the seek clamp reads this.
    fn measure_playable(&mut self) {
        let Some(trim) = self.trim else {
            return;
        };
        let shape = self.cursor.shape();
        let head = self.head_duration();
        self.total_duration = match trim.playable {
            Some(playable) => Some(frames_to_duration(playable, shape.rate)),
            None => self.total_duration.map(|total| total.saturating_sub(head)),
        };
        self.remaining = trim.playable.map(|playable| interleaved(playable, shape.channels));
    }

    /// The timestamp a post-seek trim measures against.
    ///
    /// Normally the one the demuxer echoed back, that being its own restatement of what it was
    /// asked for. Where a pre-roll was taken off the ask the two part company, and the trim still
    /// owes the target: restating it here is what turns the frames in between from a head the
    /// listener would hear into warm-up the decoder needs. A timebase that cannot restate it
    /// leaves the echoed one standing, and the seek keeps whatever warm-up its landing gave it.
    fn trim_target(&self, target: Duration, pre_roll: Duration, echoed: Timestamp) -> Timestamp {
        if pre_roll.is_zero() {
            return echoed;
        }
        let (Some(time_base), Some(time)) = (self.time_base, seek_time(target)) else {
            return echoed;
        };
        time_base.calc_timestamp(time).unwrap_or(echoed)
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
        let shape = self.cursor.shape();
        // A demuxer that landed *past* what was asked for has nothing to trim, and `Timestamp` is
        // signed, so the negative case falls out here rather than being handled below.
        let Ok(ahead) = u64::try_from(required.get().saturating_sub(actual.get())) else {
            return 0;
        };
        // Rounded up: a trim short by a frame replays the tail of the packet the seek landed in.
        let Some(frames) = decode::ticks_to_frames(ahead, time_base, shape.rate, Rounding::Up)
        else {
            return 0;
        };
        let Ok(frames) = usize::try_from(frames) else {
            return 0;
        };
        frames.saturating_mul(usize::from(shape.channels.get()))
    }

    /// Discard `count` interleaved samples, decoding as far as it takes.
    fn skip(&mut self, count: usize) {
        for _ in 0..count {
            if self.next().is_none() {
                return;
            }
        }
    }

    /// How far the demuxer's timeline runs ahead of the one the source hands out.
    fn head_duration(&self) -> Duration {
        self.trim
            .map_or(Duration::ZERO, |trim| frames_to_duration(trim.head, self.cursor.shape().rate))
    }

    /// Interleaved samples of real audio left once `pos` on the trimmed timeline has played.
    fn playable_after(&self, pos: Duration) -> Option<u64> {
        let shape = self.cursor.shape();
        let playable = self.trim?.playable?;
        let played = frames_in(pos, shape.rate);
        Some(interleaved(playable.saturating_sub(played), shape.channels))
    }
}

/// How long the file at `path` plays for, or `None` when it names no duration or no decoder is
/// registered for its codec.
///
/// The scan path's answer of last resort. Lofty reads duration off the same parse that reads the
/// tags, so a file it can't identify (a Matroska or CAF one, say) reaches the database with no
/// length at all unless someone asks the decoder instead (`media::ingest::metadata`). It costs a probe plus
/// one decoded packet, which is why it stays on that failure path rather than running for every
/// file scanned.
pub fn probe_duration(path: &Path) -> Option<Duration> {
    FileDecoder::open(path).ok()?.total_duration()
}

impl Iterator for FileDecoder {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // The trailing padding is inside the last packet rather than beyond it, so the source ends
        // on a count instead of on the demuxer running out. Saturated at zero, so a re-poll past
        // the end stays ended the way the cursor's own latch does.
        if self.remaining == Some(0) {
            return None;
        }
        let sample = self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track)?;
        if let Some(remaining) = &mut self.remaining {
            *remaining -= 1;
        }
        Some(sample)
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

        // The demuxer's timeline still opens on the encoder's priming, so a position on the
        // trimmed one sits that far short of the timestamp to ask it for.
        let target = pos.saturating_add(self.head_duration());

        // Ask short of the target wherever the codec needs warming, and let the trim below take
        // the difference back off: those frames are decoded and discarded, which is the whole of
        // what asking early buys. Zero everywhere but Opus, so no other format moves.
        let pre_roll = opus::seek_pre_roll(self.decoder.codec_params());
        let time = seek_time(target.saturating_sub(pre_roll))
            .ok_or_else(|| other(AppError::Player("Seek position out of range".to_owned())))?;

        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track) })
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

        // Cleared around the skip below, which pulls through `next`: the count still describes the
        // position being left, and one that ran out there would end the skip early and leave the
        // channel phase unrestored.
        self.remaining = None;

        // A demuxer seek lands on a packet boundary, so without the trim every seek replays the
        // tail of what came before. Both reference players stop at the whole packet, and one says
        // in its own comment that it should not. rodio trimmed to the frame, and that is the
        // behaviour this path inherited and has to keep.
        let trim = self.samples_before(
            self.trim_target(target, pre_roll, seeked.required_ts),
            seeked.actual_ts,
        );
        self.skip(trim + channel_phase);

        self.remaining = self.playable_after(pos);
        Ok(())
    }
}

fn other(source: impl std::error::Error + Send + Sync + 'static) -> SeekError {
    SeekError::Other(Arc::new(source))
}

/// `pos` as the `Time` a demuxer seek takes, or `None` where it does not fit one.
fn seek_time(pos: Duration) -> Option<Time> {
    Time::try_new(i64::try_from(pos.as_secs()).ok()?, pos.subsec_nanos())
}

#[cfg(test)]
#[path = "tests/file_decode_tests.rs"]
mod tests;
