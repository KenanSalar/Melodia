//! The sample vocabulary the DSP chain is written against, so an [`AudioSource`] is something
//! `playback::output` can pull rather than something a dependency happens to accept.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// One sample of one channel. `f32` throughout, so the DSP chain never converts.
pub type Sample = f32;

/// Channels per frame. Non-zero because a frame with none is not a frame, and because it divides.
pub type ChannelCount = std::num::NonZero<u16>;

/// Frames per second, per channel. Non-zero for the same reason: it is a divisor everywhere.
pub type SampleRate = std::num::NonZero<u32>;

/// What one stream of samples *is*, asked of a source, the device and a decoded packet alike.
/// Vocabulary, so here rather than in `playback::output::convert`, which only reads one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub channels: ChannelCount,
    pub rate: SampleRate,
}

/// What a decoder produced before it became [`Sample`]s, the one place that says whether reaching
/// `f32` cost anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceFormat {
    pub bits: u8,
    pub float: bool,
    /// Decoded from a lossy codec, so no path from here delivers the recording itself. Lossy and
    /// float coincide often enough to be confused, and a float WAV is neither.
    pub lossy: bool,
}

impl SourceFormat {
    /// What a source with nothing to say for itself reports.
    pub const F32: Self = Self { bits: 32, float: true, lossy: false };

    /// Whether widening to [`Sample`] is exact: `f32` carries 24 bits of integer and any float no
    /// wider than itself.
    pub fn fits_sample(self) -> bool {
        let widest = if self.float { 32 } else { 24 };
        self.bits <= widest
    }
}

/// `16-bit`, `32-bit float`: a lossy track and a 32-bit integer file share only a width.
impl fmt::Display for SourceFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-bit", self.bits)?;
        if self.float {
            f.write_str(" float")?;
        }
        Ok(())
    }
}

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Frames of a source at `rate` that `span` is worth, rounded **down**, with seconds and
/// nanoseconds apart so a rate not dividing a second can't cost a frame.
///
/// [`frames_to_duration`] is the way back but not an inverse: both floor, so a round trip can lose
/// up to a frame, downward, which the transport's whole-millisecond positions never notice.
/// `melodia-playback`'s `player::playback::dsp::tests` pins both halves.
pub fn frames_in(span: Duration, rate: SampleRate) -> u64 {
    let rate = u64::from(rate.get());
    let subsec = u64::from(span.subsec_nanos()) * rate / NANOS_PER_SEC;
    span.as_secs().saturating_mul(rate).saturating_add(subsec)
}

/// How long `frames` at `rate` play for — [`frames_in`]'s counterpart, with the same flooring.
pub fn frames_to_duration(frames: u64, rate: SampleRate) -> Duration {
    let rate = u64::from(rate.get());
    let nanos = (frames % rate) * NANOS_PER_SEC / rate;
    Duration::new(frames / rate, u32::try_from(nanos).unwrap_or(0))
}

/// `frames` as interleaved samples, saturating: a length read out of a corrupt container states
/// whatever it likes.
pub fn interleaved(frames: u64, channels: ChannelCount) -> u64 {
    frames.saturating_mul(u64::from(channels.get()))
}

/// Why a [`AudioSource::try_seek`] could not land.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SeekError {
    /// The source has nowhere to seek to — a live mount, or a wrapper over one.
    #[error("seeking is not supported by source: {underlying_source}")]
    NotSupported {
        /// What refused, for the log line.
        underlying_source: &'static str,
    },
    /// Anything the decoder itself raised.
    #[error(transparent)]
    Other(Arc<dyn std::error::Error + Send + Sync + 'static>),
}

/// A pullable stream of interleaved samples that knows its own shape. `Send` as a supertrait, its
/// one consumer being the audio callback thread.
pub trait AudioSource: Iterator<Item = Sample> + Send {
    /// Channels per frame. Constant for the life of the source — a mount whose shape changes under
    /// a reconnect ends instead, since the deck fixed its converter when the source was appended.
    fn channels(&self) -> ChannelCount;

    /// Frames per second, per channel, on the source's own timeline. Playback speed is applied
    /// below this, by the deck's converter, so it does not appear here.
    fn sample_rate(&self) -> SampleRate;

    /// The two above together, which is what a converter is built against.
    fn shape(&self) -> Shape {
        Shape { channels: self.channels(), rate: self.sample_rate() }
    }

    /// What the decoder at the bottom of this source produced. A wrapper forwards it.
    fn format(&self) -> SourceFormat;

    /// Whether this source hands on anything other than the samples it pulled. Required rather
    /// than defaulted, as is [`Self::format`]: a wrapper that forgot to forward would report a
    /// processed signal as untouched.
    fn dsp_engaged(&self) -> bool;

    /// How long the source runs for, when it is the kind of thing that ends.
    fn total_duration(&self) -> Option<Duration>;

    /// Seek to `pos` on the source's own timeline.
    ///
    /// # Errors
    ///
    /// [`SeekError::NotSupported`] when the source has no timeline to seek on, or whatever the
    /// decoder raised.
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError>;

    /// Gives up whatever the app reads *liveness* from, ahead of the drop. A spent source is freed
    /// away from the audio callback and outlives its last sample, and the visualizer mixes every
    /// claimed ring, so a claim held until collection leaves a finished track's tail in the window.
    /// The default releases nothing; a wrapper forwards.
    fn release_claims(&mut self) {}
}
