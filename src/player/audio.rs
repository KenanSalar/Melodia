//! The sample vocabulary the DSP chain is written against.
//!
//! Every type here was `rodio`'s until the mixer became ours, and each is spelled the same way it
//! was — `f32` samples, `NonZero` counts — so the chain that reads them did not have to change to
//! stop naming that crate. What it buys is that the chain now describes itself: an [`AudioSource`]
//! is something [`super::output`] can pull, rather than something a dependency happens to accept.

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
/// carrying their own pair before this. It lives here rather than in [`super::output::convert`]
/// because it is vocabulary — a converter is one of the things that reads a shape, not what a shape
/// belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    pub channels: ChannelCount,
    pub rate: SampleRate,
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
}
