//! Turning a live mount's bytes into samples: opening it.
//!
//! What is this module's own is everything a mount has and a file does not — no length, no seek,
//! no end of its own, and a shape that can change under a reconnect. The probe, the codec registry
//! and the packet cursor are [`super::decode`]'s, shared with [`super::file_decode`], and the
//! argument for decoding against Symphonia 0.6 is in that module's `//!`; this was the path that
//! hit it first.
//!
//! [`super::prebuffer`]'s ring sits below all of it, and what reaches the deck is `f32` rather
//! than a decoder. That is to keep the network off the audio callback thread, and it holds whether
//! or not anything else in the tree decodes.

use std::io::{Read, Seek, SeekFrom};

use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::formats::FormatReader;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSource;

use melodia_core::error::AppError;

use super::audio::{Sample, Shape};
use super::decode;

/// A reader the demuxer may read but must never seek or measure.
///
/// Both answers are the shape of a live mount rather than a shortcoming: there is no end to seek
/// to, and a stated length is what sends the probe hunting for trailing metadata that will never
/// arrive. The `Seek` impl exists only because [`MediaSource`] requires it.
pub struct LiveSource<R>(pub R);

impl<R: Read> Read for LiveSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl<R: Seek> Seek for LiveSource<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for LiveSource<R> {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// A live stream's demuxer and codec, handing out interleaved samples one at a time.
///
/// The iterator ends when the mount does, or when it stops being the shape the deck was told
/// about, both of which the feed thread reads as "try reconnecting". A packet that fails to decode
/// is skipped rather than ending it: on a mount joined mid-frame the first few routinely do.
pub struct StreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    cursor: decode::Cursor,
}

impl StreamDecoder {
    /// Probe `source`, build a decoder for its audio track, and decode enough of it to know the
    /// shape of what follows.
    pub fn open(source: Box<dyn MediaSource>, mime: Option<&str>) -> Result<Self, AppError> {
        // Passed for the same reason the content type is read at all, though 0.6 still resolves
        // the format by scoring rather than by hint.
        let mut hint = Hint::new();
        if let Some(mime) = mime {
            hint.mime_type(mime);
        }

        // The length and timebase [`decode::open`] also answers are a file's; a mount states
        // neither, so they come back empty and are dropped here.
        let opened = decode::open(source, &hint)
            .map_err(|e| AppError::Player(format!("The station's stream {e}")))?;

        Ok(Self {
            format: opened.format,
            decoder: opened.decoder,
            track: opened.track,
            cursor: opened.cursor,
        })
    }

    /// The shape a deck builds its converter from — pinned at the open, since a mount that
    /// renegotiates one mid-stream ends instead.
    pub fn shape(&self) -> Shape {
        self.cursor.shape()
    }
}

impl Iterator for StreamDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track)
    }
}
