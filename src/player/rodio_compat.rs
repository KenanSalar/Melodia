//! Temporary: lets an [`AudioSource`] be appended to a rodio deck.
//!
//! The DSP chain names [`super::audio`] from here on, but the mixer is still rodio's until
//! [`super::output`] replaces it, and rodio will only take a `rodio::Source`. This wrapper is the
//! whole of that gap and goes with the mixer.

use std::time::Duration;

use rodio::source::SeekError as RodioSeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

use super::audio::{AudioSource, SeekError};

/// Samples between the mixer re-reading the source's format.
///
/// rodio rebuilds its rate converter at a span boundary and only notices a format change at one,
/// so a short span costs the rebuild often and a long one plays that many samples of an incoming
/// source at the outgoing one's rate. This is rodio's own worst-case span (`queue::threshold`),
/// which is what every station already runs at.
const SPAN_SAMPLES: usize = 512;

/// An [`AudioSource`] wearing rodio's `Source`.
pub struct RodioBridge<S> {
    input: S,
    span: usize,
}

impl<S: AudioSource> RodioBridge<S> {
    pub fn new(input: S) -> Self {
        // Rounded up to a whole frame: a boundary landing mid-frame shears rodio's channel
        // converter, and leaves the deck's parity flipped for whatever plays next.
        let channels = usize::from(input.channels().get());
        let span = SPAN_SAMPLES.div_ceil(channels) * channels;
        Self { input, span }
    }
}

impl<S: AudioSource> Iterator for RodioBridge<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.input.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}

impl<S: AudioSource> Source for RodioBridge<S> {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        // Never `None`: that reaches `UniformSourceIterator::bootstrap` as an unbounded `Take`, so
        // the mixer builds one converter out of whichever source reached the deck first and never
        // gets a boundary to rebuild it at.
        Some(self.span)
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.input.channels()
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.input.sample_rate()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), RodioSeekError> {
        self.input.try_seek(pos).map_err(|e| match e {
            SeekError::NotSupported { underlying_source } => {
                RodioSeekError::NotSupported { underlying_source }
            }
            SeekError::Other(source) => RodioSeekError::Other(source),
        })
    }
}
