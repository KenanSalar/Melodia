//! Opening a live mount: no length, no seek, no end of its own, and a shape that can change under
//! a reconnect. Everything else is [`super::decode`]'s, shared with [`super::file_decode`].

use std::io::{Read, Seek, SeekFrom};

use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder};
use symphonia::core::formats::FormatReader;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSource;

use melodia_core::error::AppError;

use super::audio::{Sample, Shape, SourceFormat};
use super::decode;

/// How far reading may run ahead of decoding. An ICY title fires when reading reaches it while the
/// ring measures delay only after decoding, so whatever is read ahead lands early, and Symphonia's
/// uncapped 32 KiB refill is seconds of audio at a common bitrate.
const MAX_READ_AHEAD: usize = 4 * 1024;

/// Never seekable and of no length: a mount has no end to seek to, and a stated length sends the
/// probe hunting for trailing metadata that never arrives. `Seek` only because [`MediaSource`]
/// requires it.
pub struct LiveSource<R>(pub R);

impl<R: Read> Read for LiveSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = buf.len().min(MAX_READ_AHEAD);
        self.0.read(&mut buf[..len])
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

/// Ends when the mount does or stops being the shape the deck was told about, both of which the
/// feed thread reads as "try reconnecting".
pub struct StreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    cursor: decode::Cursor,
    source_format: SourceFormat,
}

impl StreamDecoder {
    pub fn open(source: Box<dyn MediaSource>, mime: Option<&str>) -> Result<Self, AppError> {
        // Passed anyway, though 0.6 resolves the format by scoring rather than by hint.
        let mut hint = Hint::new();
        if let Some(mime) = mime {
            hint.mime_type(mime);
        }

        let opened =
            decode::open(source, &hint).map_err(|e| AppError::player("The station's stream", e))?;

        Ok(Self {
            format: opened.format,
            decoder: opened.decoder,
            track: opened.track,
            cursor: opened.cursor,
            source_format: opened.source_format,
        })
    }

    /// Pinned at the open; a mount that renegotiates mid-stream ends instead.
    pub fn shape(&self) -> Shape {
        self.cursor.shape()
    }

    /// As of the open. A reconnect keeps the first answer, the deck playing one `PrebufferSource`
    /// across all of them.
    pub fn format(&self) -> SourceFormat {
        self.source_format
    }

    /// What is actually inside the container, as against what the content type named. Asked of the
    /// decoder rather than stored, being read once per open.
    pub fn codec(&self) -> AudioCodecId {
        self.decoder.codec_params().codec
    }
}

impl Iterator for StreamDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        self.cursor.next_sample(&mut *self.format, &mut *self.decoder, self.track)
    }
}

#[cfg(test)]
#[path = "tests/stream_decode_tests.rs"]
mod tests;
