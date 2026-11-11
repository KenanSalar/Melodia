//! A FLAC's metadata blocks, walked by their headers: lofty reads only the comments and pictures,
//! so the native CUESHEET block, and whether a comment names a sheet, are read here.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use melodia_core::entities::cue::EmbeddedSource;
use melodia_core::error::AppError;

const MAGIC: &[u8; 4] = b"fLaC";
const ID3V2_MAGIC: &[u8; 3] = b"ID3";
/// The `ID3v2` flag saying a footer as long as the header follows the tag.
const ID3V2_FOOTER_FLAG: u8 = 0x10;
const ID3V2_FOOTER_BYTES: i64 = 10;

const STREAMINFO: u8 = 0;
const VORBIS_COMMENT: u8 = 4;
const CUESHEET: u8 = 5;

/// Where STREAMINFO's 20-bit sample rate starts.
const STREAMINFO_RATE_AT: usize = 10;

/// The largest CUESHEET block read. A hundred tracks of 99 index points each fit, so a larger one
/// is something else, never read whole.
const MAX_CUESHEET_BYTES: u32 = 128 * 1024;

/// The comment key a sheet's text sits under, with the `=` that ends a key.
const SHEET_KEY: &[u8; 9] = b"CUESHEET=";

/// The native CUESHEET block of the FLAC at `path`, or `None` where it has none. Blocking.
pub(super) fn read_block(path: &Path) -> Result<Option<EmbeddedSource>, AppError> {
    let found = walk(path, |chain, header, sample_rate| {
        if header.kind != CUESHEET {
            return Ok(Visit::Skip);
        }
        if header.length > MAX_CUESHEET_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "CUESHEET block too large"));
        }
        let bytes = chain.body(header.length)?;
        Ok(Visit::Found(
            sample_rate.map(|sample_rate| EmbeddedSource::Block { sample_rate, bytes }),
        ))
    })?;
    Ok(found.flatten())
}

/// Whether the FLAC at `path` carries a sheet: a CUESHEET block, or a comment under the sheet's
/// key. Reads the headers and the comments' keys, nothing else. Blocking.
pub fn carries_sheet(path: &Path) -> Result<bool, AppError> {
    let found = walk(path, |chain, header, _| match header.kind {
        CUESHEET => Ok(Visit::Found(())),
        VORBIS_COMMENT if chain.comments_name_sheet(header.length)? => Ok(Visit::Found(())),
        VORBIS_COMMENT => Ok(Visit::Read),
        _ => Ok(Visit::Skip),
    })?;
    Ok(found.is_some())
}

/// What a walk does with a block.
enum Visit<T> {
    /// Stop with this answer.
    Found(T),
    /// Move past the block, which the visit left unread.
    Skip,
    /// Move on, the visit having read the block through.
    Read,
}

/// Visits each block of the FLAC at `path` until `visit` finds what it wants, handing it the
/// stream's sample rate once STREAMINFO, the block a FLAC opens with, is read.
fn walk<T>(
    path: &Path,
    mut visit: impl FnMut(&mut Chain, Header, Option<u32>) -> io::Result<Visit<T>>,
) -> Result<Option<T>, AppError> {
    let mut read = || -> io::Result<Option<T>> {
        let mut chain = Chain::open(path)?;
        let mut sample_rate = None;
        loop {
            let header = chain.header()?;
            if header.kind == STREAMINFO {
                sample_rate = stream_rate(&chain.body(header.length)?);
            } else {
                match visit(&mut chain, header, sample_rate)? {
                    Visit::Found(found) => return Ok(Some(found)),
                    Visit::Skip => chain.skip(header.length)?,
                    Visit::Read => {}
                }
            }
            if header.is_last {
                return Ok(None);
            }
        }
    };
    read().map_err(|e| AppError::metadata(format!("Failed to read {}", path.display()), e))
}

/// STREAMINFO's sample rate, `None` where it states none.
fn stream_rate(streaminfo: &[u8]) -> Option<u32> {
    let [high, middle, low] = *streaminfo.get(STREAMINFO_RATE_AT..)?.first_chunk::<3>()?;
    let rate = (u32::from(high) << 12) | (u32::from(middle) << 4) | (u32::from(low) >> 4);
    (rate > 0).then_some(rate)
}

#[derive(Clone, Copy)]
struct Header {
    is_last: bool,
    kind: u8,
    length: u32,
}

/// A FLAC's metadata, read from just past its `fLaC`.
struct Chain {
    reader: BufReader<File>,
}

impl Chain {
    fn open(path: &Path) -> io::Result<Self> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut magic = [0; 4];
        reader.read_exact(&mut magic)?;
        if magic.starts_with(ID3V2_MAGIC) {
            skip_id3v2(&mut reader)?;
            reader.read_exact(&mut magic)?;
        }
        if &magic != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a FLAC stream"));
        }
        Ok(Self { reader })
    }

    fn header(&mut self) -> io::Result<Header> {
        let mut bytes = [0; 4];
        self.reader.read_exact(&mut bytes)?;
        Ok(Header {
            is_last: bytes[0] & 0x80 != 0,
            kind: bytes[0] & 0x7F,
            length: u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]),
        })
    }

    fn body(&mut self, length: u32) -> io::Result<Vec<u8>> {
        let mut body = Vec::new();
        let read = (&mut self.reader).take(u64::from(length)).read_to_end(&mut body)?;
        if u64::try_from(read).ok() != Some(u64::from(length)) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(body)
    }

    fn skip(&mut self, length: u32) -> io::Result<()> {
        self.reader.seek_relative(i64::from(length))
    }

    /// Whether a `VORBIS_COMMENT` block of `length` bytes holds a sheet. Each comment's key is
    /// compared and the rest of it skipped, and a block naming none is read through.
    fn comments_name_sheet(&mut self, length: u32) -> io::Result<bool> {
        let mut block = (&mut self.reader).take(u64::from(length));
        let vendor = read_u32_le(&mut block)?;
        skip_bytes(&mut block, u64::from(vendor))?;
        let comments = read_u32_le(&mut block)?;
        for _ in 0..comments {
            let mut left = u64::from(read_u32_le(&mut block)?);
            if let Some(rest) = left.checked_sub(SHEET_KEY.len() as u64) {
                let mut key = [0; SHEET_KEY.len()];
                block.read_exact(&mut key)?;
                if key.eq_ignore_ascii_case(SHEET_KEY) {
                    return Ok(true);
                }
                left = rest;
            }
            skip_bytes(&mut block, left)?;
        }
        let rest = block.limit();
        skip_bytes(&mut block, rest)?;
        Ok(false)
    }
}

/// Moves `reader` past the `ID3v2` tag its first three bytes opened.
fn skip_id3v2(reader: &mut BufReader<File>) -> io::Result<()> {
    let mut header = [0; 7];
    reader.read_exact(&mut header)?;
    let [_major, _revision, flags, size @ ..] = header;
    // Seven bits a byte, the high bit clear so no byte of the size reads as a frame sync.
    let size = size.iter().fold(0_i64, |size, &byte| (size << 7) | i64::from(byte & 0x7F));
    let footer = if flags & ID3V2_FOOTER_FLAG == 0 { 0 } else { ID3V2_FOOTER_BYTES };
    reader.seek_relative(size + footer)
}

fn read_u32_le(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn skip_bytes(reader: &mut impl Read, count: u64) -> io::Result<()> {
    let skipped = io::copy(&mut reader.take(count), &mut io::sink())?;
    if skipped < count {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(())
}
