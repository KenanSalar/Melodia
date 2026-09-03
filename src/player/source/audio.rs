//! The sample vocabulary the DSP chain is written against.
//!
//! Every type here was `rodio`'s until the mixer became ours, and each is spelled the same way it
//! was — `f32` samples, `NonZero` counts — so the chain that reads them did not have to change to
//! stop naming that crate. What it buys is that the chain now describes itself: an [`AudioSource`]
//! is something `playback::output` can pull, rather than something a dependency happens to accept.

use std::sync::Arc;
use std::time::Duration;

/// One sample of one channel. `f32` throughout, so the DSP chain never converts.
pub type Sample = f32;

/// Channels per frame. Non-zero because a frame with none is not a frame, and because it divides.
pub type ChannelCount = std::num::NonZero<u16>;

/// Frames per second, per channel. Non-zero for the same reason: it is a divisor everywhere.
pub type SampleRate = std::num::NonZero<u32>;

/// What one stream of samples *is*: how many channels a frame holds, and how many frames a second.
///
/// Asked of a source, of the device, and of a decoded packet, which is three places that were each
/// carrying their own pair before this. It lives here rather than in `playback::output::convert`
/// because it is vocabulary — a converter is one of the things that reads a shape, not what a shape
/// belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub channels: ChannelCount,
    pub rate: SampleRate,
}

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Frames of a source running at `rate` that `span` is worth, rounded **down**.
///
/// Seconds and nanoseconds separately, so a rate that does not divide a second evenly cannot cost
/// the answer a frame the way a float round trip or a microsecond truncation would.
///
/// [`frames_to_duration`] is the way back but not an inverse: both floor, so a value off a frame
/// boundary loses its remainder and a round trip loses it twice. The bound is one frame, downward
/// — which is all the transport needs, every `Duration` it produces being a whole number of
/// milliseconds, and dozens of frames at any rate. `player::tests::dsp_tests` pins both halves.
pub(crate) fn frames_in(span: Duration, rate: SampleRate) -> u64 {
    let rate = u64::from(rate.get());
    let subsec = u64::from(span.subsec_nanos()) * rate / NANOS_PER_SEC;
    span.as_secs().saturating_mul(rate).saturating_add(subsec)
}

/// How long `frames` at `rate` play for — [`frames_in`]'s counterpart, with the same flooring.
pub(crate) fn frames_to_duration(frames: u64, rate: SampleRate) -> Duration {
    let rate = u64::from(rate.get());
    let nanos = (frames % rate) * NANOS_PER_SEC / rate;
    Duration::new(frames / rate, u32::try_from(nanos).unwrap_or(0))
}

/// `frames` as interleaved samples.
///
/// Saturating, because the only bound on a length read back out of a container is what fits a
/// `u64`, and a corrupt one states whatever it likes.
///
/// The third term of the vocabulary this file is — frames, the time they play for, and the samples
/// they occupy — which is why it sits beside its two siblings rather than beside its one caller.
pub(crate) fn interleaved(frames: u64, channels: ChannelCount) -> u64 {
    frames.saturating_mul(u64::from(channels.get()))
}

/// Why a [`AudioSource::try_seek`] could not land.
///
/// Two variants rather than rodio's five: the three it kept for its own bundled decoders describe
/// decoders this tree does not have.
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

/// A pullable stream of interleaved samples that knows its own shape.
///
/// The `Send` is a supertrait rather than a bound at each use site because there is only one
/// consumer and it is the audio callback thread: a source that cannot cross to it cannot play.
pub trait AudioSource: Iterator<Item = Sample> + Send {
    /// Channels per frame. Constant for the life of the source — a mount whose shape changes under
    /// a reconnect ends instead, since the deck fixed its converter when the source was appended.
    fn channels(&self) -> ChannelCount;

    /// Frames per second, per channel, on the source's own timeline. Playback speed is applied
    /// below this, by the deck's converter, so it does not appear here.
    fn sample_rate(&self) -> SampleRate;

    /// The two above together, which is what a converter is built against.
    fn shape(&self) -> Shape {
        Shape {
            channels: self.channels(),
            rate: self.sample_rate(),
        }
    }

    /// How long the source runs for, when it is the kind of thing that ends.
    fn total_duration(&self) -> Option<Duration>;

    /// Seek to `pos` on the source's own timeline.
    ///
    /// # Errors
    ///
    /// [`SeekError::NotSupported`] when the source has no timeline to seek on, or whatever the
    /// decoder raised.
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError>;

    /// Give up anything the rest of the app reads *liveness* from, ahead of the drop.
    ///
    /// A spent source is freed away from the audio callback, which means it outlives the moment it
    /// stopped playing. Whatever answers "is this deck still making sound" cannot wait for that:
    /// the visualizer mixes every claimed ring, so a claim held until collection leaves a finished
    /// track's tail in the window. Releasing is a counter decrement, and freeing is what gets
    /// deferred.
    ///
    /// Default is nothing, which is right for every source that owns no such claim. A wrapper owes
    /// a forward to whatever it wraps.
    fn release_claims(&mut self) {}
}
