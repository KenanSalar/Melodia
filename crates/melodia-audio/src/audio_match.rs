//! Whether a rewrite kept a file's audio: the copy demuxed beside the original, packet for packet.
//! Packets, not bytes, so a writer that renumbers pages or moves boxes still matches, and one that
//! drops audio or leaves an offset stale doesn't.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, Track};
use symphonia::core::packet::Packet;

use crate::player::source::decode;
use crate::player::source::file_source::{self, FileSource};

/// `ID3v1`'s fixed length, opening on `TAG`.
const ID3V1_LEN: u64 = 128;

/// An `APEv2` footer's length, and its header's.
const APE_FOOTER_LEN: u64 = 32;

const APE_HAS_HEADER: u32 = 1 << 31;

#[derive(Debug)]
pub enum AudioMatch {
    Same,
    /// The original won't demux to audio, so there is nothing to hold the copy to.
    Unchecked(SymphoniaError),
    Changed(AudioChange),
}

#[derive(Debug, thiserror::Error)]
pub enum AudioChange {
    #[error("the copy can't be read: {0}")]
    Unreadable(#[source] SymphoniaError),
    #[error("the copy's tracks differ from the original's")]
    Tracks,
    #[error("packet {0} differs from the original's")]
    Packet(u64),
    #[error("the copy ends differently at packet {0}")]
    Ending(u64),
}

/// Holds the audio of the file at `copy` to the one at `original`. Blocking, reading both whole.
pub fn compare(original: &Path, copy: &Path) -> AudioMatch {
    // The original's serves both, the copy being a temp with no extension.
    let hint = file_source::extension_hint(original);
    let mut original = match open_audio(original, &hint) {
        Ok(format) => format,
        Err(e) => return AudioMatch::Unchecked(e),
    };
    let mut copy = match open(copy, &hint) {
        Ok(format) => format,
        Err(e) => return AudioMatch::Changed(AudioChange::Unreadable(e)),
    };
    match same_packets(&mut *original, &mut *copy) {
        Ok(()) => AudioMatch::Same,
        Err(change) => AudioMatch::Changed(change),
    }
}

/// [`open`] for the original. With no audio track, both sides would read no packets and match.
fn open_audio(path: &Path, hint: &Hint) -> Result<Box<dyn FormatReader>, SymphoniaError> {
    let format = open(path, hint)?;
    if decode::audio_track(&*format).is_none() {
        return Err(SymphoniaError::Unsupported("no audio track"));
    }
    Ok(format)
}

fn open(path: &Path, hint: &Hint) -> Result<Box<dyn FormatReader>, SymphoniaError> {
    let mut file = File::open(path)?;
    let end = audio_end(&mut file)?;
    decode::probe(Box::new(FileSource::ending_at(file, end)), hint)
}

/// Where `file`'s audio ends: ahead of a trailing `ID3v1` tag and an `APEv2` tag before it, which a
/// rewrite re-serializes and an MPEG demuxer would read as frames. Leaves `file` at its start.
fn audio_end(file: &mut File) -> std::io::Result<u64> {
    let mut end = file.seek(SeekFrom::End(0))?;
    if let Some(start) = end.checked_sub(ID3V1_LEN) {
        let mut marker = [0; 3];
        read_at(file, start, &mut marker)?;
        if &marker == b"TAG" {
            end = start;
        }
    }
    if let Some(start) = end.checked_sub(APE_FOOTER_LEN) {
        let mut footer = [0; 32];
        read_at(file, start, &mut footer)?;
        if let Some(tag_start) = ape_tag_len(&footer).and_then(|len| end.checked_sub(len)) {
            end = tag_start;
        }
    }
    file.rewind()?;
    Ok(end)
}

fn read_at(file: &mut File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

/// The whole `APEv2` tag's length, header included, where `footer` is one's footer.
fn ape_tag_len(footer: &[u8; 32]) -> Option<u64> {
    let (preamble, rest) = footer.split_first_chunk::<8>()?;
    let (_version, rest) = rest.split_first_chunk::<4>()?;
    let (items_and_footer, rest) = rest.split_first_chunk::<4>()?;
    let (_item_count, rest) = rest.split_first_chunk::<4>()?;
    let (flags, _reserved) = rest.split_first_chunk::<4>()?;
    if preamble != b"APETAGEX" {
        return None;
    }
    let header = if u32::from_le_bytes(*flags) & APE_HAS_HEADER == 0 { 0 } else { APE_FOOTER_LEN };
    Some(u64::from(u32::from_le_bytes(*items_and_footer)) + header)
}

fn same_packets(
    original: &mut dyn FormatReader,
    copy: &mut dyn FormatReader,
) -> Result<(), AudioChange> {
    same_tracks(original, copy)?;
    let mut index = 0;
    loop {
        match (original.next_packet(), copy.next_packet()) {
            (Ok(Some(packet)), Ok(Some(copied))) => {
                if !same_packet(&packet, &copied) {
                    return Err(AudioChange::Packet(index));
                }
            }
            // A chained Ogg's next link, whose tracks replace the last one's.
            (Err(SymphoniaError::ResetRequired), Err(SymphoniaError::ResetRequired)) => {
                same_tracks(original, copy)?;
            }
            (Ok(None), Ok(None)) => return Ok(()),
            // A damaged original fails its copy at the same packet.
            (Err(error), Err(copied)) if same_error(&error, &copied) => return Ok(()),
            _ => return Err(AudioChange::Ending(index)),
        }
        index += 1;
    }
}

fn same_tracks(original: &dyn FormatReader, copy: &dyn FormatReader) -> Result<(), AudioChange> {
    let (original, copy) = (original.tracks(), copy.tracks());
    let same = original.len() == copy.len() && original.iter().zip(copy).all(same_track);
    same.ok_or(AudioChange::Tracks)
}

/// Compares what a decoder is built from as well, a header the writer re-packs landing there
/// rather than in a packet.
fn same_track((original, copy): (&Track, &Track)) -> bool {
    let same_params = match (&original.codec_params, &copy.codec_params) {
        (Some(CodecParameters::Audio(params)), Some(CodecParameters::Audio(copied))) => {
            params.codec == copied.codec
                && params.sample_rate == copied.sample_rate
                && params.channels == copied.channels
                && params.extra_data == copied.extra_data
        }
        (params, copied) => {
            params.as_ref().map(std::mem::discriminant)
                == copied.as_ref().map(std::mem::discriminant)
        }
    };
    original.id == copy.id && original.time_base == copy.time_base && same_params
}

fn same_packet(original: &Packet, copy: &Packet) -> bool {
    original.track_id == copy.track_id
        && original.pts == copy.pts
        && original.dts == copy.dts
        && original.dur == copy.dur
        && original.trim_start == copy.trim_start
        && original.trim_end == copy.trim_end
        && original.data == copy.data
}

fn same_error(original: &SymphoniaError, copy: &SymphoniaError) -> bool {
    match (original, copy) {
        (SymphoniaError::IoError(error), SymphoniaError::IoError(copied)) => {
            error.kind() == copied.kind()
        }
        (SymphoniaError::DecodeError(message), SymphoniaError::DecodeError(copied))
        | (SymphoniaError::Unsupported(message), SymphoniaError::Unsupported(copied))
        | (SymphoniaError::LimitError(message), SymphoniaError::LimitError(copied)) => {
            message == copied
        }
        _ => false,
    }
}
