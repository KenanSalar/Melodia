//! Opening a file and the frame-accurate seek: what a file has and a mount doesn't. The probe, the
//! registry and the cursor are [`super::decode`]'s, shared with [`super::stream_decode`].

use std::collections::VecDeque;
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

use super::aac_trim;
use super::audio::{
    AudioSource, ChannelCount, Sample, SampleRate, SeekError, SourceFormat, frames_in,
    frames_to_duration, interleaved,
};
use super::decode::{self, Rounding};
use super::mkv_trim;
use super::opus;

/// How far short of a stated length a seek lands. The length is not somewhere to land: some
/// formats answer out of range and park at the end, and the deck reads that as the track finishing,
/// which a drag to the slider's right edge asks for exactly. Clears the gap between a stated length
/// and the last decodable frame, which tags overstate routinely, and still sounds like the end.
const SEEK_END_MARGIN: Duration = Duration::from_millis(100);

/// A file with its seekability and length taken once: Symphonia's own `File` source re-reads the
/// filesystem on every call, which the demuxer makes per probe and per seek. No `BufReader`,
/// [`MediaSourceStream`] already being the read-ahead buffer.
struct FileSource {
    inner: File,
    seekable: bool,
    byte_len: Option<u64>,
}

impl FileSource {
    fn new(inner: File) -> Self {
        // Anything but a regular file is a pipe or a device wearing an audio extension.
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

/// How much of the container's timeline isn't music, filled by [`super::aac_trim`], the Opus arm
/// of [`FileDecoder::install_trim`] and [`super::mkv_trim`].
#[derive(Clone, Copy)]
pub(super) struct Trim {
    /// Encoder priming, ahead of the first real sample.
    pub head: u64,
    /// Real audio after it, where the container states a length.
    pub playable: Option<u64>,
    /// Padding the container states inside its last packets, where nothing below drops it.
    pub tail: u64,
}

/// What stops this source short of the demuxer running out. Never both: a container stating a
/// length states it short of the padding already, so a window behind it would take the frames off
/// twice, and one field makes that a property of the type.
enum End {
    /// Interleaved samples of real audio left, where the container states a length.
    Counted(u64),
    /// The trailing padding held back, where all the container states is how much of it there is.
    Held(TailWindow),
}

/// Holds the trailing padding back: a container states its length too coarsely to end on a count,
/// so every sample is delayed by the padding's width, and what is left inside when the decoder runs
/// out is what the container asked to drop.
struct TailWindow {
    held: VecDeque<Sample>,
    width: usize,
}

impl TailWindow {
    /// `None` with nothing to hold back. Sized for the sample arriving before the oldest leaves.
    fn new(width: u64) -> Option<Self> {
        let width = usize::try_from(width).ok().filter(|width| *width > 0)?;
        Some(Self { held: VecDeque::with_capacity(width + 1), width })
    }

    /// The sample that arrived `width` pulls before `sample`, or `None` while still filling.
    fn push(&mut self, sample: Sample) -> Option<Sample> {
        self.held.push_back(sample);
        if self.held.len() <= self.width {
            return None;
        }
        self.held.pop_front()
    }
}

/// A file's demuxer and codec, handing out interleaved samples one at a time.
pub struct FileDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    cursor: decode::Cursor,
    source_format: SourceFormat,
    time_base: Option<TimeBase>,
    total_duration: Option<Duration>,
    /// For the seek, which has to add the head back on.
    trim: Option<Trim>,
    end: Option<End>,
}

impl FileDecoder {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let mut file = File::open(path)
            .map_err(|e| AppError::io(format!("Cannot open {}", path.display()), e))?;

        // While the handle is still ours: an MP4 edit list and Matroska discard padding are both
        // what a container states and the demuxer keeps to itself.
        let edits = aac_trim::edit_lists(&mut file);
        let discards = mkv_trim::discard_padding(&mut file);

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(extension);
        }

        let decode::Opened {
            format,
            decoder,
            track,
            cursor,
            source_format,
            time_base,
            total_duration,
        } = decode::open(Box::new(FileSource::new(file)), &hint)
            .map_err(|e| AppError::player(path.display().to_string(), e))?;

        let mut decoded = Self {
            format,
            decoder,
            track,
            cursor,
            source_format,
            time_base,
            total_duration,
            trim: None,
            end: None,
        };
        decoded.install_trim(&edits, &discards);
        Ok(decoded)
    }

    /// Lines this source's timeline up with the demuxer's. AAC's head comes off through
    /// [`Self::skip`], above the cursor the stream decoder shares; [`super::opus`] has already
    /// dropped Opus's, leaving an offset; a tail is the container's, which only
    /// [`super::mkv_trim`] answers. Never on the audio callback, every caller opening off the deck
    /// lock. Here rather than in [`super::decode`]: a mount has no container header to read a trim
    /// from and no gapless seam for one to spoil.
    fn install_trim(&mut self, edits: &[aac_trim::Edit], discards: &[mkv_trim::Discard]) {
        let tail = mkv_trim::resolve(discards, self.track, self.cursor.shape().rate).unwrap_or(0);

        if let Some(aac) = self.aac_padding(edits) {
            let shape = self.cursor.shape();
            self.trim = Some(Trim { tail, ..aac });
            // Ahead of the counts, which describe the audio left once this is gone.
            self.skip(usize::try_from(interleaved(aac.head, shape.channels)).unwrap_or(usize::MAX));
            self.measure_playable();
            log::debug!(
                "AAC encoder padding: {} priming frames dropped, {:?} of audio",
                aac.head,
                self.total_duration
            );
            return;
        }

        let head = self.opus_priming().unwrap_or(0);
        if head == 0 && tail == 0 {
            return;
        }

        self.trim = Some(Trim { head, playable: None, tail });
        self.measure_playable();
        log::debug!(
            "Container padding: {head} priming frames already off the samples, {tail} trailing \
             frames held back, {:?} of audio",
            self.total_duration
        );
    }

    fn aac_padding(&mut self, edits: &[aac_trim::Edit]) -> Option<Trim> {
        let timing = decode::audio_track(&*self.format).and_then(aac_trim::aac_timing)?;
        aac_trim::resolve(&timing, &self.format.metadata(), edits, self.cursor.shape().rate)
    }

    /// The priming [`super::opus`] already dropped, which the demuxer's timeline still counts.
    /// Gated on the codec rather than the field: MP3 and CAF fill `Track::delay` too, and a blanket
    /// rule would take their delay off twice. Only Ogg fills it for Opus; Matroska subtracts
    /// `CodecDelay` from its timestamps instead and MP4 states nothing.
    fn opus_priming(&self) -> Option<u64> {
        let track = decode::audio_track(&*self.format)?;
        if decode::audio_params(track)?.codec != CODEC_ID_OPUS {
            return None;
        }
        track.delay.map(u64::from)
    }

    /// Restates the length, and arms the end, against the real audio. The tail owes no
    /// subtraction: a container's own duration ends where its padding starts.
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
        self.end = self.end_after(Duration::ZERO);
    }

    /// Where a pre-roll was taken off the ask, the trim still owes the target, which turns the
    /// frames between into warm-up rather than a head the listener hears. Otherwise, or where the
    /// timebase can't restate the target, the demuxer's echo stands.
    fn trim_target(&self, target: Duration, pre_roll: Duration, echoed: Timestamp) -> Timestamp {
        if pre_roll.is_zero() {
            return echoed;
        }
        let (Some(time_base), Some(time)) = (self.time_base, seek_time(target)) else {
            return echoed;
        };
        time_base.calc_timestamp(time).unwrap_or(echoed)
    }

    /// Interleaved samples between where the demuxer landed and where the seek asked, rounded up to
    /// a whole frame so a seek can't replay one already heard. Integer against the timebase's own
    /// ratio, so an uneven rate can't drift the answer by a frame.
    fn samples_before(&self, required: Timestamp, actual: Timestamp) -> usize {
        let Some(time_base) = self.time_base else {
            return 0;
        };
        let shape = self.cursor.shape();
        // `Timestamp` is signed, so a landing *past* the ask falls out here as nothing to trim.
        let Ok(ahead) = u64::try_from(required.get().saturating_sub(actual.get())) else {
            return 0;
        };
        let Some(frames) = decode::ticks_to_frames(ahead, time_base, shape.rate, Rounding::Up)
        else {
            return 0;
        };
        let Ok(frames) = usize::try_from(frames) else {
            return 0;
        };
        frames.saturating_mul(usize::from(shape.channels.get()))
    }

    /// Discards `count` interleaved samples, decoding as far as it takes.
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

    /// The end the padding calls for once `pos` has played: a count restated from there, or a
    /// window rebuilt empty, what it held describing the position being left.
    fn end_after(&self, pos: Duration) -> Option<End> {
        let trim = self.trim?;
        let shape = self.cursor.shape();
        let Some(playable) = trim.playable else {
            return TailWindow::new(interleaved(trim.tail, shape.channels)).map(End::Held);
        };
        let played = frames_in(pos, shape.rate);
        Some(End::Counted(interleaved(playable.saturating_sub(played), shape.channels)))
    }
}

/// How long the file at `path` plays for. The scan path's last resort for a file lofty can't read a
/// length from, Matroska or CAF say (`media::ingest::metadata`), at the cost of a probe and a
/// decoded packet.
pub fn probe_duration(path: &Path) -> Option<Duration> {
    FileDecoder::open(path).ok()?.total_duration()
}

impl Iterator for FileDecoder {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.end {
            None => self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track),
            // The padding is inside the last packet, so a stated length ends on its count.
            // Saturated at zero, so a re-poll past the end stays ended.
            Some(End::Counted(0)) => None,
            Some(End::Counted(remaining)) => {
                let sample =
                    self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track)?;
                *remaining -= 1;
                Some(sample)
            }
            // More than one turn only while the window fills, at the open and after a seek.
            Some(End::Held(window)) => loop {
                let sample =
                    self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track)?;
                if let Some(held) = window.push(sample) {
                    return Some(held);
                }
            },
        }
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

    fn format(&self) -> SourceFormat {
        self.source_format
    }

    fn dsp_engaged(&self) -> bool {
        false
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // The caller asks past the end routinely, positions coming off tags that overshoot the
        // decoded length, and the ceiling sits short of it for [`SEEK_END_MARGIN`]'s reason.
        let pos =
            self.total_duration.map_or(pos, |total| pos.min(total.saturating_sub(SEEK_END_MARGIN)));

        // The demuxer's timeline still opens on the priming.
        let target = pos.saturating_add(self.head_duration());

        // Ask short of the target where the codec needs warming, and let the trim take it back.
        // Zero everywhere but Opus, and zero without the timebase the trim is measured through,
        // asking early with nothing to trim being a replay rather than a warm-up.
        let pre_roll = match self.time_base {
            Some(_) => opus::seek_pre_roll(self.decoder.codec_params()),
            None => Duration::ZERO,
        };
        let time = seek_time(target.saturating_sub(pre_roll))
            .ok_or_else(|| other(AppError::player_msg("Seek position out of range")))?;

        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track) })
            .map_err(other)?;

        // Unconditional, as upstream asks: whatever overlap state the decoder holds now describes
        // audio that is no longer adjacent, and the stateless codecs' `reset` does nothing.
        self.decoder.reset();

        // A seek restarts on a frame boundary, so the channel a puller was part way through has to
        // be put back, or channel 0 comes out where channel 1 was due.
        let channel_phase = self.cursor.discard_buffered();

        // Dropped around the skip, which pulls through `next`: a count running out there would
        // end the skip early and leave the channel phase unrestored.
        self.end = None;

        // A demuxer seek lands on a packet boundary, so without trimming to the frame every seek
        // replays the tail of what came before.
        let trim = self.samples_before(
            self.trim_target(target, pre_roll, seeked.required_ts),
            seeked.actual_ts,
        );
        self.skip(trim + channel_phase);

        self.end = self.end_after(pos);
        Ok(())
    }
}

fn other(source: impl std::error::Error + Send + Sync + 'static) -> SeekError {
    SeekError::Other(Arc::new(source))
}

/// `None` where `pos` doesn't fit a `Time`.
fn seek_time(pos: Duration) -> Option<Time> {
    Time::try_new(i64::try_from(pos.as_secs()).ok()?, pos.subsec_nanos())
}

#[cfg(test)]
#[path = "tests/file_decode_tests.rs"]
mod tests;
