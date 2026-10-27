//! The encoder delay an AAC file states and nothing between the container and the decoder acts on
//! (`docs/adr/0011-aac-trim-read-from-the-file.md`): the MP4 demuxer fills no trim and the AAC
//! decoder ignores the gapless option.
//!
//! An `iTunSMPB` tag survives as a raw tag and is parsed; an edit list nothing exposes, so it is
//! walked for here. `iTunSMPB` wins where both are present, naming the original sample count
//! outright. Scope is AAC: the [`Trim`](super::file_decode::Trim) this resolves takes an Opus head
//! and a Matroska tail from elsewhere.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::formats::Track;
use symphonia::core::meta::{Metadata, RawValue};
use symphonia::core::units::{Duration as SymphoniaDuration, TimeBase};

use super::audio::SampleRate;
use super::file_decode::Trim;

/// The walk descends four levels into boxes a muxer writes once each, so the real count is dozens;
/// this only bounds a file claiming a million empty ones.
const MAX_BOX_HEADERS: u32 = 4096;

/// Edit list entries examined for the one that starts partway into the media.
const MAX_EDIT_ENTRIES: usize = 4;

const BOX_HEADER_LEN: u64 = 8;
const ELST_ENTRY_V0: usize = 12;
const ELST_ENTRY_V1: usize = 20;
/// Version, flags and the entry count, ahead of the first entry.
const ELST_HEADER: usize = 8;
/// Enough of a `tkhd`, `mvhd` or `mdhd` to reach the field after its two timestamps.
const HEADER_BOX_PREFIX: usize = 24;

/// Apple's default is 2112 frames and no encoder states anything near a second, so a larger head
/// is a misread, and acting on it would cut real audio off the front.
const HEAD_CEILING_SECS: u64 = 1;

/// What one track's edit list says, in that track's own media timescale.
pub(super) struct Edit {
    pub track_id: u32,
    /// Media ticks of encoder priming ahead of where the presentation starts.
    pub delay: u64,
    /// Media ticks the presentation runs for, where its timescale converts exactly
    /// ([`exact_media_ticks`]).
    pub playable: Option<u64>,
}

/// By value, reaching the reader's metadata borrowing it mutably while the track is borrowed from
/// that same reader.
pub(super) struct Timing {
    id: u32,
    time_base: TimeBase,
    /// Total media ticks, priming included.
    duration: Option<u64>,
}

/// `track`'s timeline, when it is an AAC track that states one.
pub(super) fn aac_timing(track: &Track) -> Option<Timing> {
    if super::decode::audio_params(track)?.codec != CODEC_ID_AAC {
        return None;
    }
    Some(Timing {
        id: track.id,
        time_base: track.time_base?,
        // A stated zero is the reader saying it doesn't know.
        duration: track.duration.map(SymphoniaDuration::get).filter(|ticks| *ticks > 0),
    })
}

/// The trim the container states for that track, or `None` where it states none worth acting on.
pub(super) fn resolve(
    timing: &Timing,
    metadata: &Metadata<'_>,
    edits: &[Edit],
    rate: SampleRate,
) -> Option<Trim> {
    let (head_ticks, playable_ticks) = if let Some(smpb) = smpb(metadata) {
        (smpb.priming, Some(smpb.frames))
    } else {
        let edit = edits.iter().find(|edit| edit.track_id == timing.id)?;
        // The edit's own length excludes the trailing padding the track duration can still carry;
        // the derived length is the fallback where it converts inexactly and the ceiling where not.
        let derived = timing.duration.map(|total| total.saturating_sub(edit.delay));
        let playable = match (edit.playable, derived) {
            (Some(stated), Some(derived)) => Some(stated.min(derived)),
            (stated, derived) => stated.or(derived),
        };
        (edit.delay, playable)
    };

    let head = to_frames(head_ticks, timing.time_base, rate)?;
    let playable = match playable_ticks {
        Some(ticks) => Some(to_frames(ticks, timing.time_base, rate)?),
        None => None,
    };

    if head > u64::from(rate.get()) * HEAD_CEILING_SECS || playable == Some(0) {
        return None;
    }
    // The tail is the container's answer, filled in beside this.
    (head > 0 || playable.is_some()).then_some(Trim { head, playable, tail: 0 })
}

/// `rate` is the **decoder's**: [`super::aac_config`] demotes HE-AAC to a core at half the declared
/// rate the edit list is written against, and `iTunSMPB` counts samples at that declared rate too.
/// Rounded **down**, since a head that overshoots cuts real audio and a length that overshoots
/// keeps the padding.
fn to_frames(ticks: u64, time_base: TimeBase, rate: SampleRate) -> Option<u64> {
    super::decode::ticks_to_frames(ticks, time_base, rate, super::decode::Rounding::Down)
}

/// The priming and the original sample count, as `iTunSMPB` spells them.
struct Smpb {
    priming: u64,
    frames: u64,
}

/// The key arrives as its mean and name joined by a colon, matched case-insensitively, the
/// spelling being a convention rather than a registered identifier.
fn smpb(metadata: &Metadata<'_>) -> Option<Smpb> {
    let tag = metadata.current()?.media.tags.iter().find(|tag| {
        tag.raw.key.rsplit(':').next().is_some_and(|name| name.eq_ignore_ascii_case("iTunSMPB"))
    })?;
    let RawValue::String(value) = &tag.raw.value else {
        return None;
    };
    parse_smpb(value)
}

/// Ten to twelve hex fields, of which the priming, the remainder and the original sample count
/// carry anything; the remainder is skipped, the count already bounding the tail.
fn parse_smpb(value: &str) -> Option<Smpb> {
    let mut fields = value.split_ascii_whitespace().skip(1);
    let priming = u64::from_str_radix(fields.next()?, 16).ok()?;
    let frames = u64::from_str_radix(fields.nth(1)?, 16).ok()?;
    (frames > 0).then_some(Smpb { priming, frames })
}

/// Every edit list in the file, keyed by track: an audiobook's chapter track or a `.m4v`'s video
/// one has its own. Rewinds the handle, which the caller hands on to the demuxer.
pub(super) fn edit_lists(file: &mut File) -> Vec<Edit> {
    let mut edits = Vec::new();
    let Ok(end) = file.metadata().map(|meta| meta.len()) else {
        return edits;
    };

    let mut budget = MAX_BOX_HEADERS;
    walk(file, 0, end, &mut budget, |file, moov, budget| {
        if moov.kind != *b"moov" {
            return;
        }
        // A pass of its own, an edit length being stated against this and box order not being
        // something a file owes us.
        let mut movie_timescale = None;
        walk(file, moov.payload, moov.end, budget, |file, mvhd, _| {
            if mvhd.kind == *b"mvhd" {
                movie_timescale = header_u32(file, mvhd);
            }
        });

        walk(file, moov.payload, moov.end, budget, |file, trak, budget| {
            if trak.kind == *b"trak"
                && let Some(edit) = trak_edit(file, trak, budget, movie_timescale)
            {
                edits.push(edit);
            }
        });
    });

    let _ = file.seek(SeekFrom::Start(0));
    edits
}

/// A box's type and the span its payload occupies.
struct BoxHeader {
    kind: [u8; 4],
    payload: u64,
    end: u64,
}

/// Hands `visit` each box between `pos` and `end`, stopping at the first that doesn't parse: a
/// short read is a malformed file the demuxer is about to reject anyway.
fn walk(
    file: &mut File,
    mut pos: u64,
    end: u64,
    budget: &mut u32,
    mut visit: impl FnMut(&mut File, &BoxHeader, &mut u32),
) {
    while pos.saturating_add(BOX_HEADER_LEN) <= end && *budget > 0 {
        *budget -= 1;
        let Some(header) = read_header(file, pos, end) else {
            return;
        };
        pos = header.end;
        visit(file, &header, budget);
    }
}

fn read_header(file: &mut File, pos: u64, end: u64) -> Option<BoxHeader> {
    let mut head = [0u8; 8];
    read_at(file, pos, &mut head)?;

    let kind = [head[4], head[5], head[6], head[7]];
    let (size, payload) = match be_u32(&head, 0)? {
        // A size of one puts the real one in the eight bytes after the type.
        1 => {
            let mut extended = [0u8; 8];
            read_at(file, pos + BOX_HEADER_LEN, &mut extended)?;
            (u64::from_be_bytes(extended), pos + 16)
        }
        // A size of zero means the box runs to the end of its parent.
        0 => (end.checked_sub(pos)?, pos + BOX_HEADER_LEN),
        stated => (u64::from(stated), pos + BOX_HEADER_LEN),
    };

    let box_end = pos.checked_add(size)?;
    // Forward and inside its parent, so a size of the file's own choosing can neither loop the walk
    // nor send it past the end.
    (box_end <= end && payload <= box_end).then_some(BoxHeader { kind, payload, end: box_end })
}

/// The edit `trak` states, paired with its track id; both are required, the presentation length
/// not, a delay being worth having without one.
fn trak_edit(
    file: &mut File,
    trak: &BoxHeader,
    budget: &mut u32,
    movie_timescale: Option<u32>,
) -> Option<Edit> {
    let mut track_id = None;
    let mut media_timescale = None;
    let mut first = None;

    walk(file, trak.payload, trak.end, budget, |file, child, budget| match &child.kind {
        b"tkhd" => track_id = header_u32(file, child),
        b"mdia" => walk(file, child.payload, child.end, budget, |file, mdhd, _| {
            if mdhd.kind == *b"mdhd" {
                media_timescale = header_u32(file, mdhd);
            }
        }),
        b"edts" => walk(file, child.payload, child.end, budget, |file, elst, _| {
            if elst.kind == *b"elst" {
                first = first_edit(file, elst);
            }
        }),
        _ => {}
    });

    let first = first?;
    let playable = movie_timescale
        .zip(media_timescale)
        .and_then(|(movie, media)| exact_media_ticks(first.segment_duration, movie, media));

    Some(Edit { track_id: track_id?, delay: first.media_time, playable })
}

/// A movie-timescale count in media ticks, or `None` where it would round. Divisibility rather
/// than equal timescales: ffmpeg's 1000 against 44100 rarely divides, and one that does is still
/// the only length excluding the trailing padding, [`resolve`] capping it at the derived one.
fn exact_media_ticks(ticks: u64, movie_timescale: u32, media_timescale: u32) -> Option<u64> {
    let scaled = u128::from(ticks) * u128::from(media_timescale);
    let movie = u128::from(movie_timescale);
    if movie == 0 || scaled % movie != 0 {
        return None;
    }
    u64::try_from(scaled / movie).ok()
}

/// The `u32` after a header box's two timestamps: a `tkhd`'s track id, an `mvhd` or `mdhd`'s
/// timescale.
fn header_u32(file: &mut File, header: &BoxHeader) -> Option<u32> {
    let stated = usize::try_from(header.end.checked_sub(header.payload)?).unwrap_or(usize::MAX);
    if stated < HEADER_BOX_PREFIX {
        return None;
    }

    let mut buf = [0u8; HEADER_BOX_PREFIX];
    read_at(file, header.payload, &mut buf)?;

    // The version picks the width of the two timestamps sitting ahead of the field.
    let at = match buf[0] {
        0 => 12,
        1 => 20,
        _ => return None,
    };
    be_u32(&buf, at).filter(|stated| *stated > 0)
}

/// One edit list entry: where in the media it starts, and how long it presents for.
struct ElstEntry {
    /// Media ticks.
    media_time: u64,
    /// Movie ticks.
    segment_duration: u64,
}

/// The first edit that presents media, which is where a muxer writes the priming. Scanned, an
/// empty edit (media time -1) being able to sit ahead of the real one.
fn first_edit(file: &mut File, elst: &BoxHeader) -> Option<ElstEntry> {
    let mut buf = [0u8; ELST_HEADER + MAX_EDIT_ENTRIES * ELST_ENTRY_V1];
    let stated = usize::try_from(elst.end.checked_sub(elst.payload)?).unwrap_or(buf.len());
    let read = stated.min(buf.len());
    if read < ELST_HEADER {
        return None;
    }
    read_at(file, elst.payload, buf.get_mut(..read)?)?;

    let (width, wide) = match buf.first()? {
        0 => (ELST_ENTRY_V0, false),
        1 => (ELST_ENTRY_V1, true),
        _ => return None,
    };
    let entries = usize::try_from(be_u32(&buf, 4)?).unwrap_or(MAX_EDIT_ENTRIES);

    (0..entries.min(MAX_EDIT_ENTRIES)).find_map(|index| {
        let at = ELST_HEADER + index * width;
        if at + width > read {
            return None;
        }
        let (segment_duration, media_time) = if wide {
            (be_u64(&buf, at)?, be_i64(&buf, at + 8)?)
        } else {
            (u64::from(be_u32(&buf, at)?), i64::from(be_i32(&buf, at + 4)?))
        };
        Some(ElstEntry { media_time: u64::try_from(media_time).ok()?, segment_duration })
    })
}

fn read_at(file: &mut File, pos: u64, buf: &mut [u8]) -> Option<()> {
    file.seek(SeekFrom::Start(pos)).ok()?;
    file.read_exact(buf).ok()
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at + 4)?.try_into().ok().map(u32::from_be_bytes)
}

fn be_i32(bytes: &[u8], at: usize) -> Option<i32> {
    bytes.get(at..at + 4)?.try_into().ok().map(i32::from_be_bytes)
}

fn be_u64(bytes: &[u8], at: usize) -> Option<u64> {
    bytes.get(at..at + 8)?.try_into().ok().map(u64::from_be_bytes)
}

fn be_i64(bytes: &[u8], at: usize) -> Option<i64> {
    bytes.get(at..at + 8)?.try_into().ok().map(i64::from_be_bytes)
}

#[cfg(test)]
#[path = "tests/aac_trim_tests.rs"]
mod tests;
