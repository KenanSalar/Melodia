//! A file's end of the ring that keeps a blocking read off the audio callback thread: a
//! `decode-ahead` thread owns the [`FileDecoder`] and keeps the ring topped up, so a slow drive
//! stalls only the deck reading it. That deck holds rather than playing silence as music
//! ([`AudioSource::is_ready`]), its clock standing still until audio flows again.

use std::time::Duration;

use melodia_core::error::AppError;

use super::audio::{
    AudioSource, ChannelCount, Sample, SampleRate, SeekError, Shape, SourceFormat, frames_in,
    interleaved,
};
use super::file_decode::FileDecoder;
use super::sample_ring::{self, FeedState, Pulled, RingReader, RingWriter};

/// How much decoded audio a file keeps ahead of its deck: the longest read it rides out before the
/// deck holds, and the memory each playing or staged file costs.
pub const DEPTH: Duration = Duration::from_secs(2);

/// The least a ring holds before its deck pulls it. Covers one mixer step at the top speed plus
/// the resampler's lookahead at the lowest rates a file and a device meet at, so a step started
/// above it never runs the ring dry.
const LOW_WATER: Duration = Duration::from_millis(100);

/// A full ring's feeder naps for this share of its depth: it wakes rarely, and refills long before
/// a deck at the top speed could drain it to [`LOW_WATER`].
const NAPS_PER_DEPTH: u32 = 4;

pub struct DecodeAhead {
    reader: RingReader,
    shape: Shape,
    format: SourceFormat,
    dsp_engaged: bool,
    total_duration: Option<Duration>,
    /// [`LOW_WATER`] as interleaved samples of this source.
    low_water: usize,
}

impl DecodeAhead {
    /// Decodes on the caller's thread until the ring holds `depth` or the file ends, then leaves the
    /// rest to a feeder, so a mount or a seek never starts on a hold and a logged hold is a stall.
    ///
    /// # Errors
    ///
    /// [`AppError::Io`] where the feeder thread can't be started.
    pub fn start(mut decoder: FileDecoder, depth: Duration) -> Result<Self, AppError> {
        let shape = decoder.shape();
        let (writer, reader) =
            sample_ring::ring(shape, depth, depth / NAPS_PER_DEPTH, FeedState::new());
        let low_water = interleaved(frames_in(LOW_WATER.min(depth), shape.rate), shape.channels);
        let ahead = Self {
            reader,
            shape,
            format: decoder.format(),
            dsp_engaged: decoder.dsp_engaged(),
            total_duration: decoder.total_duration(),
            low_water: usize::try_from(low_water).unwrap_or(usize::MAX),
        };

        if fill(&mut decoder, &writer) {
            spawn_feeder(decoder, writer)?;
        } else {
            writer.finish();
        }
        Ok(ahead)
    }
}

/// Decodes until the ring is full, `false` where the file ended first.
fn fill(decoder: &mut FileDecoder, writer: &RingWriter) -> bool {
    while !writer.is_full() {
        let Some(sample) = decoder.next() else {
            return false;
        };
        writer.push(sample);
    }
    true
}

fn spawn_feeder(decoder: FileDecoder, writer: RingWriter) -> Result<(), AppError> {
    // A literal, which `crates/melodia/tests/packaging.rs`'s thread-name walk reads.
    let _detached = std::thread::Builder::new()
        .name("decode-ahead".to_owned())
        .spawn(move || feed(decoder, &writer))
        .map_err(|e| AppError::io("Cannot start the decode-ahead thread", e))?;
    Ok(())
}

/// Tops the ring up until the file ends or the source is dropped. The decoder, and with it the
/// file, is freed here rather than wherever the source is.
fn feed(decoder: FileDecoder, writer: &RingWriter) {
    for sample in decoder {
        if !writer.push(sample) {
            return;
        }
    }
    writer.finish();
}

impl Iterator for DecodeAhead {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        match self.reader.pull() {
            Pulled::Sample(sample) => Some(sample),
            // Only for a deck pulling past `is_ready`; silence keeps the frame whole.
            Pulled::Starved => Some(0.0),
            Pulled::Ended => None,
        }
    }
}

impl AudioSource for DecodeAhead {
    #[inline]
    fn channels(&self) -> ChannelCount {
        self.shape.channels
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.shape.rate
    }

    fn format(&self) -> SourceFormat {
        self.format
    }

    fn dsp_engaged(&self) -> bool {
        self.dsp_engaged
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    /// The ring already holds what follows the old position, so a seek starts a new source.
    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        Err(SeekError::NotSupported { underlying_source: "file decoded ahead" })
    }

    fn is_ready(&self) -> bool {
        self.reader.is_finished() || self.reader.buffered() >= self.low_water
    }

    /// Stops the feeder when the deck lets go, rather than when the source is next collected.
    fn release_claims(&mut self) {
        self.reader.abandon();
    }
}
