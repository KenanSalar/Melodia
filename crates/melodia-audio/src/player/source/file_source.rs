//! A file as Symphonia reads it, its seekability and length taken once: Symphonia's own `File`
//! source re-reads the filesystem on every call, which the demuxer makes per probe and per seek. No
//! `BufReader`, `MediaSourceStream` already being the read-ahead buffer.

use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSource;

pub(crate) fn extension_hint(path: &Path) -> Hint {
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    hint
}

pub(crate) struct FileSource {
    inner: File,
    seekable: bool,
    byte_len: Option<u64>,
    /// Where reads stop short of the file's own end, set by [`Self::ending_at`].
    end: Option<u64>,
    position: u64,
}

impl FileSource {
    pub(crate) fn new(inner: File) -> Self {
        // Anything but a regular file is a pipe or a device wearing an audio extension.
        let regular = inner.metadata().ok().filter(Metadata::is_file);
        Self {
            seekable: regular.is_some(),
            byte_len: regular.map(|m| m.len()),
            end: None,
            position: 0,
            inner,
        }
    }

    /// `inner`, standing at its start, read as though it ended at `end`: past it a demuxer would
    /// take trailing tags for audio.
    pub(crate) fn ending_at(inner: File, end: u64) -> Self {
        Self { end: Some(end), ..Self::new(inner) }
    }
}

impl Read for FileSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let buf = match self.end {
            Some(end) => {
                let left = usize::try_from(end.saturating_sub(self.position)).unwrap_or(usize::MAX);
                let len = buf.len().min(left);
                &mut buf[..len]
            }
            None => buf,
        };
        let read = self.inner.read(buf)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for FileSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let pos = match (pos, self.end) {
            (SeekFrom::End(offset), Some(end)) => {
                let target = end.checked_add_signed(offset);
                SeekFrom::Start(target.ok_or(std::io::ErrorKind::InvalidInput)?)
            }
            _ => pos,
        };
        self.position = self.inner.seek(pos)?;
        Ok(self.position)
    }
}

impl MediaSource for FileSource {
    fn is_seekable(&self) -> bool {
        self.seekable
    }

    fn byte_len(&self) -> Option<u64> {
        self.end.or(self.byte_len)
    }
}
