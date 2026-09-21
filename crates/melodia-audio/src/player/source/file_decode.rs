//! Turning a file's bytes into samples: opening it, and the seek.
//!
//! What is this module's own is everything a file has and a mount does not — a length, a seek that
//! can land anywhere, and an end it reaches by itself. The probe, the codec registry and the packet
//! cursor are [`super::decode`]'s, shared with [`super::stream_decode`], and the argument for
//! decoding against Symphonia 0.6 rather than the 0.5 rodio pinned is in that module's `//!`.
//!
//! It replaced `rodio::Decoder`, so that type doubles as the specification: what must not be lost
//! is the frame-accurate seek, which is the one thing here neither reference implementation does.

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
    AudioSource, ChannelCount, Sample, SampleRate, SeekError, frames_in, frames_to_duration,
    interleaved,
};
use super::decode::{self, Rounding};
use super::mkv_trim;
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

/// How much of what the container's timeline counts is not music.
///
/// The reader's rather than any codec's, three sources filling one and none of them reading it back.
/// [`super::aac_trim`] resolves the head and the length off what an AAC file states, the Opus arm of
/// [`FileDecoder::install_trim`] fills a head off the offset [`super::opus`] leaves behind having
/// already dropped the samples, and [`super::mkv_trim`] answers the tail for whatever codec a
/// Matroska file carries.
#[derive(Clone, Copy)]
pub(super) struct Trim {
    /// Encoder priming, ahead of the first real sample.
    pub head: u64,
    /// Real audio after it, where the container states a length.
    pub playable: Option<u64>,
    /// Padding the container states inside its last packets, where nothing below drops it.
    pub tail: u64,
}

/// What stops this source short of the demuxer running out.
///
/// Never both at once: a container that states a length states it short of the padding already, so
/// a window behind one would take the same frames off twice. One field rather than two is what
/// makes that a property of the type instead of an invariant spread across three call sites.
enum End {
    /// Interleaved samples of real audio left, where the container states a length.
    Counted(u64),
    /// The trailing padding held back, where all the container states is how much of it there is.
    Held(TailWindow),
}

/// Interleaved samples held back so a container's trailing padding is never handed out.
///
/// The count to drop is known at the open and the count of samples ahead of it is not: a container
/// states its own length far more coarsely than a frame, so ending on a count the way
/// [`Trim::playable`] does would leave part of the padding in. A window this wide delays every
/// sample by exactly as many instead, and whatever is still inside it when the decoder runs out is
/// what the container asked to have dropped.
struct TailWindow {
    held: VecDeque<Sample>,
    width: usize,
}

impl TailWindow {
    /// A window `width` interleaved samples wide, or `None` where there is nothing to hold back.
    ///
    /// The capacity covers the one sample that arrives before the oldest leaves, so the queue never
    /// grows past the open.
    fn new(width: u64) -> Option<Self> {
        let width = usize::try_from(width).ok().filter(|width| *width > 0)?;
        Some(Self { held: VecDeque::with_capacity(width + 1), width })
    }

    /// Takes `sample` and hands back the one that arrived `width` pulls before it, or `None` while
    /// the window is still filling.
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
    time_base: Option<TimeBase>,
    total_duration: Option<Duration>,
    /// The encoder padding this file states, for the seek that has to add its head back on.
    trim: Option<Trim>,
    /// How that padding's trailing half is kept out of what this hands over.
    end: Option<End>,
}

impl FileDecoder {
    /// Probe `path`, build a decoder for its audio track, and decode enough of it to know the
    /// shape of what follows.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let mut file = File::open(path)
            .map_err(|e| AppError::Player(format!("Cannot open {}: {e}", path.display())))?;

        // Read while the handle is still ours: an MP4's edit list and a Matroska file's discard
        // padding are both halves of what a container states and the demuxer keeps to itself, and
        // either way it is the same open. Each costs one read for a file of the other's container.
        let edits = aac_trim::edit_lists(&mut file);
        let discards = mkv_trim::discard_padding(&mut file);

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
            end: None,
        };
        decoded.install_trim(&edits, &discards);
        Ok(decoded)
    }

    /// Lines this source's timeline up with the demuxer's, dropping the priming where nothing
    /// below has.
    ///
    /// A head arrives in one of two states and a tail only ever in one. AAC states its head where no
    /// decoder reads it, so that one comes off through [`Self::skip`] rather than a window inside the
    /// cursor: the cursor is shared with the stream decoder, and a live mount has neither a container
    /// header stating a delay nor a gapless transition to spoil. Opus states its head in the
    /// identification packet and [`super::opus`] has already taken it off, so all that is left is the
    /// offset it leaves behind on a timeline that still counts those frames. A tail is the
    /// container's, not the codec's, and [`super::mkv_trim`] is the only reader that answers one.
    ///
    /// Runs on the thread that opened the file, which is never the audio callback: all three call
    /// sites hoist the open off the deck lock for the position monitor's sake.
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
    /// instead, so its timeline already agrees with the samples, and MP4 fills nothing. That is the
    /// head alone. Matroska states its tail in an element the reader drops, which is
    /// [`super::mkv_trim`]'s half.
    fn opus_priming(&self) -> Option<u64> {
        let track = decode::audio_track(&*self.format)?;
        if decode::audio_params(track)?.codec != CODEC_ID_OPUS {
            return None;
        }
        track.delay.map(u64::from)
    }

    /// Restates the length, and arms the end, against the real audio rather than against everything
    /// the container holds.
    ///
    /// A file stating a head and no length still plays for that much less than the container says,
    /// and the seek clamp reads this. The tail owes no such subtraction: a container's own duration
    /// ends where the padding starts, which is the element's definition and the same reason [`End`]
    /// is one field.
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

    /// The timestamp a post-seek trim measures against.
    ///
    /// Normally the one the demuxer echoed back, that being its own restatement of what it was
    /// asked for. Where a pre-roll was taken off the ask the two part company, and the trim still
    /// owes the target: restating it here is what turns the frames in between from a head the
    /// listener would hear into warm-up the decoder needs. A target the timebase cannot restate
    /// leaves the echoed one standing, which is the seek this path made before there was a
    /// pre-roll to take off.
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

    /// The end this file's padding calls for, as it stands once `pos` on the trimmed timeline has
    /// played.
    ///
    /// A count is restated from there. A window is rebuilt empty and refills on the next pull,
    /// whatever it held having described the position being left rather than what precedes the next
    /// sample handed out.
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
        match &mut self.end {
            None => self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track),
            // The trailing padding is inside the last packet rather than beyond it, so a source
            // with a stated length ends on that count instead of on the demuxer running out.
            // Saturated at zero, so a re-poll past the end stays ended the way the cursor's own
            // latch does.
            Some(End::Counted(0)) => None,
            Some(End::Counted(remaining)) => {
                let sample =
                    self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track)?;
                *remaining -= 1;
                Some(sample)
            }
            // More than one turn only while the window is filling, which is at the open and again
            // after a seek.
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
        // what asking early buys. Zero everywhere but Opus, so no other format moves, and zero
        // without the timebase the trim is measured through, since asking early with nothing to
        // trim is a replay rather than a warm-up.
        let pre_roll = match self.time_base {
            Some(_) => opus::seek_pre_roll(self.decoder.codec_params()),
            None => Duration::ZERO,
        };
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

        // Dropped around the skip below, which pulls through `next`: it still describes the
        // position being left, and a count that ran out there would end the skip early and leave
        // the channel phase unrestored.
        self.end = None;

        // A demuxer seek lands on a packet boundary, so without the trim every seek replays the
        // tail of what came before. Both reference players stop at the whole packet, and one says
        // in its own comment that it should not. rodio trimmed to the frame, and that is the
        // behaviour this path inherited and has to keep.
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

/// `pos` as the `Time` a demuxer seek takes, or `None` where it does not fit one.
fn seek_time(pos: Duration) -> Option<Time> {
    Time::try_new(i64::try_from(pos.as_secs()).ok()?, pos.subsec_nanos())
}

#[cfg(test)]
#[path = "tests/file_decode_tests.rs"]
mod tests;
