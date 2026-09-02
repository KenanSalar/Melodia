//! The encoder delay an AAC file states, and that nothing between the container and the decoder
//! acts on.
//!
//! Symphonia 0.6 does gapless at the decoder, under `AudioDecoderOptions::gapless`, and only its
//! MP3 and Vorbis decoders act on it. `symphonia-format-isomp4` parses the edit list into an atom
//! it never reads back, fills neither `Track::delay` nor `Track::padding`, and builds packets
//! through a constructor that zeroes both trims; `symphonia-codec-aac` takes the options and
//! ignores them. So the numbers reach nothing, and reading them back is ours to do. Neither
//! reference player does it either.
//!
//! Two places state them and a file carries one or both. iTunes, qaac and Apple Music write an
//! `iTunSMPB` tag, which Symphonia does surface: a freeform key it has no mapping for survives as a
//! raw tag, so that half is a string to parse rather than a box to find. ffmpeg and most other
//! muxers write an edit list instead, and nothing exposes that, which is what the walk below is
//! for. `iTunSMPB` wins where both are present: it names the original sample count outright, where
//! an edit list only offsets a duration the container states elsewhere.
//!
//! Scope is AAC. FLAC and ALAC are lossless and pad nothing, Vorbis is trimmed upstream, and Opus
//! has no decoder here yet.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::formats::Track;
use symphonia::core::meta::{Metadata, RawValue};
use symphonia::core::units::{Duration as SymphoniaDuration, TimeBase};

use super::audio::SampleRate;

/// Box headers the walk will read before giving up.
///
/// It descends four levels into boxes a muxer writes once each, so the real count is in the dozens.
/// The budget only bounds a file claiming a million empty ones, which would otherwise cost a seek
/// and a read per eight bytes of it.
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

/// The longest priming worth believing, in seconds.
///
/// Apple's default is 2112 frames and no encoder states anything near a second. A larger number is
/// a misread, or a muxer using the edit list for something other than encoder delay, and acting on
/// it would cut real audio off the front of the track.
const HEAD_CEILING_SECS: u64 = 1;

/// What one track's edit list says, in that track's own media timescale.
pub(super) struct Edit {
    pub track_id: u32,
    /// Media ticks of encoder priming ahead of where the presentation starts.
    pub delay: u64,
    /// Media ticks the presentation runs for, where the edit list's own timescale converts to this
    /// one exactly. See [`exact_media_ticks`] for why an inexact one is dropped rather than rounded.
    pub playable: Option<u64>,
}

/// Frames the decoder hands over that are not music.
#[derive(Clone, Copy)]
pub(super) struct Trim {
    /// Encoder priming, dropped ahead of the first real sample.
    pub head: u64,
    /// Real audio after it, where the container states a length.
    pub playable: Option<u64>,
}

/// What a track's own timeline is measured in.
///
/// Taken by value because reaching the reader's metadata borrows it mutably, and the track is
/// borrowed from that same reader.
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
        // A stated zero is the reader saying it doesn't know, as it is everywhere else this asks.
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
        // The edit list's own length is the presentation, so it excludes the trailing padding the
        // track duration can still carry; the duration is the backstop where it converts inexactly,
        // and the ceiling where it doesn't, since no edit may present audio the track doesn't hold.
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
    (head > 0 || playable.is_some()).then_some(Trim { head, playable })
}

/// Converts a count stated against the container's timescale into decoded frames.
///
/// The two agree for almost every AAC file, and the one case where they don't is the one this
/// cannot skip: [`super::aac_config`] rewrites an HE-AAC config to its LC core, so the decoder runs
/// at half the rate the container declares, while the edit list is written against the declared one.
///
/// `iTunSMPB` counts PCM samples rather than ticks, and takes this same conversion because the
/// encoder writing the tag writes the media timescale as the sample rate. Reading it off
/// `AudioCodecParameters::sample_rate` instead would not be safer: that field is the `stsd` entry's
/// 16.16 rate, which for HE-AAC names the core layer about as often as the doubled one.
fn to_frames(ticks: u64, time_base: TimeBase, rate: SampleRate) -> Option<u64> {
    let scaled = u128::from(ticks) * u128::from(time_base.numer.get()) * u128::from(rate.get());
    u64::try_from(scaled / u128::from(time_base.denom.get())).ok()
}

/// The priming and the original sample count, as `iTunSMPB` spells them.
struct Smpb {
    priming: u64,
    frames: u64,
}

/// Reads `iTunSMPB` off the tags the demuxer collected.
///
/// The key arrives as its mean and name joined by a colon, so the name is the last segment. Matched
/// case-insensitively, the spelling being a convention rather than a registered identifier.
fn smpb(metadata: &Metadata<'_>) -> Option<Smpb> {
    let tag = metadata.current()?.media.tags.iter().find(|tag| {
        tag.raw.key.rsplit(':').next().is_some_and(|name| name.eq_ignore_ascii_case("iTunSMPB"))
    })?;
    let RawValue::String(value) = &tag.raw.value else {
        return None;
    };
    parse_smpb(value)
}

/// Parses the hex fields of an `iTunSMPB` value.
///
/// Ten to twelve whitespace-separated hex numbers, of which three carry anything: the priming, the
/// remainder, and the original sample count. The remainder is skipped because the count already
/// bounds the tail, and the trailing fields have never carried a meaning.
fn parse_smpb(value: &str) -> Option<Smpb> {
    let mut fields = value.split_ascii_whitespace().skip(1);
    let priming = u64::from_str_radix(fields.next()?, 16).ok()?;
    let frames = u64::from_str_radix(fields.nth(1)?, 16).ok()?;
    (frames > 0).then_some(Smpb { priming, frames })
}

/// Every edit list in the file, keyed by the track it belongs to.
///
/// Keyed rather than taken as the file's one answer because an MP4 may hold more than one track: an
/// audiobook carries a chapter track beside the audio, and a `.m4v` a video one, neither of whose
/// edit list means anything to the track being decoded. Cover art is not one of them, being an
/// `ilst` atom rather than a track, so no fixture here states the case and
/// `an_edit_list_belonging_to_another_track_is_not_taken_for_this_ones` builds it.
///
/// The handle is rewound before this returns, the caller handing the same one to the demuxer.
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
        // Taken in a pass of its own rather than as it comes: an edit list states its length
        // against this timescale, and box order is not something a file owes us.
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

/// Hands `visit` each box between `pos` and `end`, stopping at the first one that doesn't parse.
///
/// A short read means a truncated or malformed file rather than something to recover from, and
/// everything this walk is after sits in the header the demuxer is about to reject anyway.
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
    // The two bounds keep the walk moving forward and inside its parent, so a size field of the
    // file's own choosing can neither loop it nor send it reading past the end.
    (box_end <= end && payload <= box_end).then_some(BoxHeader {
        kind,
        payload,
        end: box_end,
    })
}

/// The edit `trak` states, paired with the track it identifies itself as.
///
/// Both halves are required: an edit list belonging to no named track cannot be matched against the
/// one being decoded, and a track stating no edit has nothing to contribute. The presentation
/// length is not: a file may state one this cannot convert, and the delay is still worth having.
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

    Some(Edit {
        track_id: track_id?,
        delay: first.media_time,
        playable,
    })
}

/// Restates a movie-timescale count in media ticks, or `None` where the conversion would round.
///
/// The rule is divisibility rather than equal timescales. A movie timescale is routinely coarser
/// than the media one, 1000 against 44100 being ffmpeg's pairing, so most segment durations do not
/// divide and the derived length answers for them instead. One that does still carries whatever the
/// muxer rounded away writing it, and is taken anyway: it is the only statement of the presentation
/// length that excludes the trailing padding, and [`resolve`] caps it at the derived one.
fn exact_media_ticks(ticks: u64, movie_timescale: u32, media_timescale: u32) -> Option<u64> {
    let scaled = u128::from(ticks) * u128::from(media_timescale);
    let movie = u128::from(movie_timescale);
    if movie == 0 || scaled % movie != 0 {
        return None;
    }
    u64::try_from(scaled / movie).ok()
}

/// The `u32` a header box states after its two timestamps: the track id in a `tkhd`, the timescale
/// in an `mvhd` or `mdhd`. All three sit at the same offset, under the same version rule.
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

/// The first edit that presents media, which is where a muxer writes the priming.
///
/// Scanned rather than taken from entry zero: an empty edit, spelled with a media time of -1, can
/// sit ahead of the real one to delay the presentation. A media time of zero is not empty, and its
/// segment duration still bounds the tail.
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
        Some(ElstEntry {
            media_time: u64::try_from(media_time).ok()?,
            segment_duration,
        })
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
