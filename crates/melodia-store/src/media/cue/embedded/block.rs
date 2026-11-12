//! FLAC's native CUESHEET block (RFC 9639 §8.7) as the sheet it states: where each track and its
//! index points sit, the disc's catalog number and each track's ISRC. It holds no text, so a title
//! comes from a `CUE_TRACKnn_TITLE` comment where there is one.

use super::super::parse::{Files, Sheet, SheetTrack};
use melodia_core::entities::cue::{CueFrames, CueTrack, FRAMES_PER_SECOND};

const CATALOG_BYTES: usize = 128;
/// The lead-in sample count, then the CD-DA flag and the reserved bytes behind it.
const AFTER_CATALOG_BYTES: usize = 8 + 259;
const ISRC_BYTES: usize = 12;
/// The track's type and pre-emphasis flags, then the reserved bytes behind them.
const TRACK_FLAGS_BYTES: usize = 14;
const NON_AUDIO_FLAG: u8 = 0x80;
const INDEX_RESERVED_BYTES: usize = 3;

/// The sheet `bytes` state, their offsets counting samples at `sample_rate`, or `None` where they
/// don't parse. The lead-out, numbered past 99, marks the end of the audio the last track runs to.
pub(super) fn sheet(sample_rate: u32, bytes: &[u8]) -> Option<Sheet> {
    let mut block = Block(bytes);
    let catalog = text(block.take(CATALOG_BYTES)?);
    block.take(AFTER_CATALOG_BYTES)?;
    let count = block.u8()?;
    let mut tracks = Vec::with_capacity(count.into());
    for _ in 0..count {
        let offset = block.u64()?;
        let number = block.u8()?;
        let isrc = text(block.take(ISRC_BYTES)?);
        let flags = *block.take(TRACK_FLAGS_BYTES)?.first()?;
        let index_count = block.u8()?;
        let mut indexes = Vec::with_capacity(index_count.into());
        for _ in 0..index_count {
            let at = offset.checked_add(block.u64()?)?;
            let index = block.u8()?;
            block.take(INDEX_RESERVED_BYTES)?;
            indexes.push((index, frames(at, sample_rate)?));
        }
        let number = CueTrack::try_from(i64::from(number)).ok();
        if let Some(number) = number.filter(|&number| number != CueTrack::HIDDEN) {
            let is_audio = flags & NON_AUDIO_FLAG == 0;
            tracks.push(SheetTrack::inside(number, is_audio, isrc, indexes));
        }
    }
    Some(Sheet { catalog, files: Files::Inside.at_start(), tracks, ..Sheet::default() })
}

/// `samples` at `sample_rate` in cue frames, rounded to the nearest: exact for a block a sheet was
/// imported into, whose offsets fall on frames.
fn frames(samples: u64, sample_rate: u32) -> Option<CueFrames> {
    let rate = u128::from(sample_rate);
    let frames = (u128::from(samples) * u128::from(FRAMES_PER_SECOND) + rate / 2) / rate;
    CueFrames::try_from(i64::try_from(frames).ok()?).ok()
}

/// A fixed-width field's text, `None` where it is blank: the format pads with NULs.
fn text(bytes: &[u8]) -> Option<String> {
    let text =
        std::str::from_utf8(bytes).ok()?.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!text.is_empty()).then(|| text.to_owned())
}

/// The block's bytes not yet read.
struct Block<'a>(&'a [u8]);

impl<'a> Block<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(*self.take(8)?.first_chunk()?))
    }
}
