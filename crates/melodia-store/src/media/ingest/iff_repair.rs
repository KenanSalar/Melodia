//! Keeping a tag write from costing a WAV or AIFF its ID3 chunk, and undoing one that did.
//!
//! lofty splices a new ID3 chunk in at the size the container states. Where that size leaves out
//! the final pad byte, the splice lands a byte early: inside the old tag chunk, whose drain then
//! cuts the new chunk's `I`, or in the last chunk's pad slot. Either way the pad strays past the new
//! chunk, and a reader walking the chunks loses the tag. [`include_final_pad`] keeps a write from
//! it; [`diagnose`] and [`repair`] undo one, matching only the shapes that splice leaves.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use lofty::file::FileType;

use super::metadata::{self, TagScope};
use super::tag_writer;
use melodia_core::error::{AppError, AppResult};
use melodia_core::utils::atomic_file;

/// A container's id, stated size and form type.
const CONTAINER_HEADER_LEN: u64 = 12;

/// Where a container states its size, after its four-byte id.
const SIZE_AT: u64 = 4;

/// A chunk's id and size, which a stated size leaves out.
const CHUNK_HEADER_LEN: u64 = 8;

/// What a cut leaves of an `ID3 ` chunk's id.
const CUT_ID: &[u8] = b"D3 ";

/// Where a cut chunk's size and `ID3v2` header sit, one byte earlier than a whole chunk's.
const CUT_SIZE_AT: u64 = 3;
const CUT_TAG_AT: u64 = 7;

const ID3V2_HEADER_LEN: u64 = 10;

/// The `ID3v2` flag for a footer, a copy of the header after the frames.
const ID3V2_FOOTER: u8 = 0x10;

/// Counts a WAV's or AIFF's final pad byte in `copy`'s container size where its header leaves it
/// out, so lofty's splice lands past the last chunk. Any other format's handle stays at its start,
/// where lofty's save probes the file from.
pub(crate) fn include_final_pad(copy: &mut File, file_type: FileType) -> io::Result<()> {
    if !matches!(file_type, FileType::Wav | FileType::Aiff) {
        return Ok(());
    }
    let Some(container) = Container::read(copy)? else {
        return Ok(());
    };
    let Some(counted) = container.stated.checked_add(1).filter(|_| container.lacks_final_pad())
    else {
        return Ok(());
    };
    write_size(copy, container.order, counted)
}

/// What [`diagnose`] found in a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A walk that reaches the end, or no RIFF or AIFF container at all.
    Clean,
    Damaged(Damage),
    /// The walk breaks at `at` in a way no tag write explains.
    Unrecognised {
        at: u64,
    },
}

/// The byte a splice cost a file and the one it left astray, as [`diagnose`] found them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Damage {
    order: ByteOrder,
    insert_at: u64,
    insert_byte: u8,
    stray_at: u64,
    /// Where the mended tag chunk ends, when later writes appended chunks of their own past it.
    chain: Option<u64>,
}

/// Reads `path`'s chunk headers, and the `ID3v2` header where one is cut. Blocking.
///
/// # Errors
///
/// A file that won't open or read.
pub fn diagnose(path: &Path) -> AppResult<Verdict> {
    let read_failed =
        |e| AppError::metadata(format!("Failed to read the chunks of {}", path.display()), e);
    let mut file = File::open(path).map_err(read_failed)?;
    diagnose_file(&mut file).map_err(read_failed)
}

/// Undoes `damage` in a copy of `path`, renamed over it only once the copy shows the same damage,
/// walks clean mended and folded, and plays as the original does. Blocking.
///
/// A damaged AIFF won't demux, so its audio goes unchecked; the span mended holds tag chunks only.
///
/// # Errors
///
/// A file that changed since [`diagnose`] read it, a failed write, or a refused copy.
pub fn repair(path: &Path, damage: Damage) -> AppResult<()> {
    let require = |copy: &mut File, expected: Verdict, failure: &str| -> AppResult<()> {
        if diagnose_file(copy)? == expected {
            return Ok(());
        }
        Err(AppError::metadata_msg(format!("{} {failure}", path.display())))
    };
    atomic_file::rewrite_with_sync(path, |copy, copy_path| {
        require(copy, Verdict::Damaged(damage), "changed since it was read")?;
        damage.mend(copy)?;
        require(copy, Verdict::Clean, "still breaks once mended")?;
        damage.fold_chain(copy, copy_path)?;
        require(copy, Verdict::Clean, "breaks once its chain is folded")?;
        tag_writer::require_same_audio(path, copy_path)
    })
}

impl Damage {
    /// Writes the missing byte in and shifts what follows, up to the stray byte, one place on over
    /// it: the file keeps its length, and nothing past the stray byte moves.
    fn mend(self, copy: &mut File) -> AppResult<()> {
        let span_len = usize::try_from(self.stray_at - self.insert_at + 1)
            .map_err(|e| AppError::metadata("A damaged tag chunk is too long to mend", e))?;
        let mut span = vec![0; span_len];
        copy.seek(SeekFrom::Start(self.insert_at))?;
        copy.read_exact(&mut span)?;
        span.copy_within(..span_len - 1, 1);
        span[0] = self.insert_byte;
        copy.seek(SeekFrom::Start(self.insert_at))?;
        copy.write_all(&span)?;

        let whole = copy.metadata()?.len();
        restate(copy, self.order, whole)
    }

    /// Saves the tag lofty merges from a chain over its first chunk and cuts the rest off: a reader
    /// may take the first ID3 chunk alone, which holds none of the writes made after the damage.
    fn fold_chain(self, copy: &mut File, copy_path: &Path) -> AppResult<()> {
        let Some(first_end) = self.chain else {
            return Ok(());
        };
        let merged = metadata::read_tags(copy_path, TagScope::Full)?;
        copy.set_len(first_end)?;
        restate(copy, self.order, first_end)?;
        tag_writer::save_tags(merged, copy)
            .map_err(|e| AppError::metadata("Failed to fold a chain of ID3 chunks", e))
    }
}

/// States the size of a container `file_len` bytes long, refusing one past what RIFF and AIFF hold.
fn restate(file: &mut File, order: ByteOrder, file_len: u64) -> AppResult<()> {
    let size = u32::try_from(file_len - CHUNK_HEADER_LEN)
        .map_err(|e| AppError::metadata("A container too large to restate", e))?;
    write_size(file, order, size)?;
    Ok(())
}

fn write_size(file: &mut File, order: ByteOrder, size: u32) -> io::Result<()> {
    file.seek(SeekFrom::Start(SIZE_AT))?;
    file.write_all(&order.write(size))
}

fn diagnose_file(file: &mut File) -> io::Result<Verdict> {
    let Some(container) = Container::read(file)? else {
        return Ok(Verdict::Clean);
    };
    let mut walk = Walk { file, order: container.order, len: container.len };
    let mut at = CONTAINER_HEADER_LEN;
    let mut previous_odd = false;
    while let Some(header) = walk.header(at)? {
        if header.id.starts_with(CUT_ID) {
            return walk.cut_chunk(at, previous_odd);
        }
        let Some(end) = walk.chunk_end(at, header) else {
            return Ok(Verdict::Unrecognised { at });
        };
        previous_odd = !header.size.is_multiple_of(2);
        at = end;
    }
    Ok(Verdict::Clean)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ByteOrder {
    /// RIFF's.
    Little,
    /// AIFF's.
    Big,
}

impl ByteOrder {
    fn read(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    fn write(self, value: u32) -> [u8; 4] {
        match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        }
    }
}

/// What a RIFF/WAVE or FORM/AIFF opening says, beside the file's length.
#[derive(Clone, Copy)]
struct Container {
    order: ByteOrder,
    stated: u32,
    len: u64,
}

impl Container {
    /// `None` for a file that opens as neither.
    fn read(file: &mut File) -> io::Result<Option<Self>> {
        let len = file.metadata()?.len();
        if len < CONTAINER_HEADER_LEN {
            return Ok(None);
        }
        let [i0, i1, i2, i3, s0, s1, s2, s3, f0, f1, f2, f3] = read_at(file, 0)?;
        let order = match (&[i0, i1, i2, i3], &[f0, f1, f2, f3]) {
            (b"RIFF", b"WAVE") => ByteOrder::Little,
            (b"FORM", b"AIFF" | b"AIFC") => ByteOrder::Big,
            _ => return Ok(None),
        };
        Ok(Some(Self { order, stated: order.read([s0, s1, s2, s3]), len }))
    }

    /// Every chunk pads to an even length, so an odd size one byte short of the file leaves out the
    /// last chunk's pad.
    fn lacks_final_pad(self) -> bool {
        !self.stated.is_multiple_of(2) && u64::from(self.stated) + CHUNK_HEADER_LEN + 1 == self.len
    }
}

#[derive(Clone, Copy)]
struct ChunkHeader {
    id: [u8; 4],
    size: u32,
}

impl ChunkHeader {
    /// Where the chunk at `at` ends, its pad byte included.
    fn end(self, at: u64) -> u64 {
        at + CHUNK_HEADER_LEN + padded(self.size)
    }
}

fn padded(size: u32) -> u64 {
    u64::from(size) + u64::from(size % 2)
}

/// A strict walk over a container's chunks, each followed by its pad as the formats lay them out.
/// A reader that forgives a missing pad would step over the very misalignment this looks for.
struct Walk<'a> {
    file: &'a mut File,
    order: ByteOrder,
    len: u64,
}

impl Walk<'_> {
    /// The header at `at`, `None` with no room left for one.
    fn header(&mut self, at: u64) -> io::Result<Option<ChunkHeader>> {
        if at.saturating_add(CHUNK_HEADER_LEN) > self.len {
            return Ok(None);
        }
        let [a, b, c, d, s0, s1, s2, s3] = read_at(self.file, at)?;
        Ok(Some(ChunkHeader { id: [a, b, c, d], size: self.order.read([s0, s1, s2, s3]) }))
    }

    /// Where the chunk at `at` ends, `None` for one whose id is malformed or whose data runs past
    /// the file.
    fn chunk_end(&self, at: u64, header: ChunkHeader) -> Option<u64> {
        let fits = at + CHUNK_HEADER_LEN + u64::from(header.size) <= self.len;
        (is_chunk_id(header.id) && fits).then(|| header.end(at))
    }

    /// Matches the chunk a splice cut at `at`, and what later writes left past it.
    fn cut_chunk(&mut self, at: u64, previous_odd: bool) -> io::Result<Verdict> {
        let unrecognised = Ok(Verdict::Unrecognised { at });
        let size = self.order.read(read_at(self.file, at + CUT_SIZE_AT)?);
        if self.id3v2_len(at + CUT_TAG_AT)? != Some(u64::from(size)) {
            return unrecognised;
        }
        // A chunk spliced into the pad slot keeps its `I` there; a drained one lost it.
        let landed_early = previous_odd && read_at(self.file, at - 1)? == *b"I";
        let (insert_at, insert_byte) = if landed_early { (at - 1, 0) } else { (at, b'I') };

        let mended_end = at + CHUNK_HEADER_LEN + padded(size);
        let Some(tail) = self.tail(mended_end - 1)? else {
            return unrecognised;
        };
        Ok(Verdict::Damaged(Damage {
            order: self.order,
            insert_at,
            insert_byte,
            stray_at: tail.stray_at,
            chain: tail.chained.then_some(mended_end),
        }))
    }

    /// The length the `ID3v2` tag at `at` states for itself, `None` where none starts there.
    fn id3v2_len(&mut self, at: u64) -> io::Result<Option<u64>> {
        if at + ID3V2_HEADER_LEN > self.len {
            return Ok(None);
        }
        let [i, d, three, major, _revision, flags, s0, s1, s2, s3] = read_at(self.file, at)?;
        let size = [s0, s1, s2, s3];
        let synchsafe = size.iter().all(|byte| byte & 0x80 == 0);
        if [i, d, three] != *b"ID3" || !(2..=4).contains(&major) || !synchsafe {
            return Ok(None);
        }
        let body = size.iter().fold(0, |len, &byte| (len << 7) | u64::from(byte));
        let footer = if flags & ID3V2_FOOTER == 0 { 0 } else { ID3V2_HEADER_LEN };
        Ok(Some(ID3V2_HEADER_LEN + body + footer))
    }

    /// Walks the ID3 and `JUNK` chunks from `at` to the end of the file, finding the one zero byte
    /// the splice left astray among them: last, or just ahead of a chunk a later write appended.
    fn tail(&mut self, mut at: u64) -> io::Result<Option<Tail>> {
        let mut stray_at = None;
        let mut chained = false;
        while at < self.len {
            if stray_at.is_none() && self.is_stray(at)? {
                stray_at = Some(at);
                at += 1;
                continue;
            }
            let Some(header) = self.header(at)? else {
                return Ok(None);
            };
            let Some(end) = self.chunk_end(at, header).filter(|_| is_tail_chunk(header.id)) else {
                return Ok(None);
            };
            chained |= is_id3_chunk(header.id);
            at = end;
        }
        Ok(stray_at.filter(|_| at == self.len).map(|stray_at| Tail { stray_at, chained }))
    }

    fn is_stray(&mut self, at: u64) -> io::Result<bool> {
        if read_at(self.file, at)? != [0] {
            return Ok(false);
        }
        let next = at + 1;
        if next == self.len {
            return Ok(true);
        }
        Ok(self.header(next)?.is_some_and(|header| {
            is_id3_chunk(header.id) && self.chunk_end(next, header).is_some()
        }))
    }
}

/// What later writes left past a cut chunk.
struct Tail {
    stray_at: u64,
    /// Whether another ID3 chunk follows the cut one.
    chained: bool,
}

/// Four printable ASCII bytes, any spaces trailing.
fn is_chunk_id(id: [u8; 4]) -> bool {
    let printable = id.iter().all(|byte| (b' '..=b'~').contains(byte));
    let spaces_trail = id.iter().skip_while(|&&byte| byte != b' ').all(|&byte| byte == b' ');
    printable && spaces_trail
}

fn is_id3_chunk(id: [u8; 4]) -> bool {
    matches!(&id, b"ID3 " | b"id3 ")
}

/// What a tag write leaves past the chunk it spliced: more ID3 chunks, and lofty's padding.
fn is_tail_chunk(id: [u8; 4]) -> bool {
    is_id3_chunk(id) || &id == b"JUNK"
}

fn read_at<const N: usize>(file: &mut File, at: u64) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    file.seek(SeekFrom::Start(at))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
#[path = "tests/iff_repair_tests.rs"]
mod tests;
