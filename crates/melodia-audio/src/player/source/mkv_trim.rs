//! The trailing padding a Matroska file states and nothing acts on: `symphonia-format-mkv` parses
//! `DiscardPadding` onto a struct it reads nowhere and zeroes both packet trims, the gap
//! [`super::aac_trim`] fills for an MP4 edit list.
//!
//! Container-scoped, applying to whatever codec the track carries; `CodecDelay` already comes off
//! the timestamps, so only the tail leaks. The walk answers in Matroska ticks, nanoseconds unscaled
//! by `TimestampScale`, and [`resolve`] converts them once the decoder's rate is known.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use symphonia::core::units::TimeBase;

use super::audio::SampleRate;

// Element ids, spelled with the length-marker bits the spec writes them with.
const EBML_HEADER: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const CLUSTER: u32 = 0x1F43_B675;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const DISCARD_PADDING: u32 = 0x75A2;

/// Matroska ticks in a second.
const TICKS_PER_SEC: u32 = 1_000_000_000;

// The widest an element id, a vint and an integer payload can be.
const MAX_ID_LEN: usize = 4;
const MAX_VINT_LEN: usize = 8;
/// Enough for the widest id and the widest size beside it.
const ELEMENT_HEADER_LEN: usize = MAX_ID_LEN + MAX_VINT_LEN;

/// The real count is in the hundreds, one per top-level element and cluster and one per element in
/// the last cluster; this only bounds a file claiming a million empty ones.
const MAX_ELEMENT_HEADERS: u32 = 8192;

/// [`super::aac_trim`]'s head ceiling at the other end: no encoder pads near a second, so more is a
/// misread that would cut real audio. Also what bounds the window holding the frames back.
const TAIL_CEILING_SECS: u64 = 1;

/// The trailing padding one track's last stating block declares, in Matroska ticks.
pub(super) struct Discard {
    pub track_num: u32,
    pub ticks: u64,
}

/// Keyed by track, a padding stated for a second audio or a subtitle track meaning nothing to the
/// one decoded; the reader's track ids are the `TrackNumber` a block names. Rewinds the handle for
/// the demuxer.
pub(super) fn discard_padding(file: &mut File) -> Vec<Discard> {
    let discards = read_discards(file).unwrap_or_default();
    let _ = file.seek(SeekFrom::Start(0));
    discards
}

/// The trailing padding stated for `track_num`, in frames decoded at `rate`.
pub(super) fn resolve(discards: &[Discard], track_num: u32, rate: SampleRate) -> Option<u64> {
    let discard = discards.iter().find(|discard| discard.track_num == track_num)?;
    let frames = ticks_to_frames(discard.ticks, rate)?;
    (frames > 0 && frames <= u64::from(rate.get()) * TAIL_CEILING_SECS).then_some(frames)
}

/// Rounded **down**: a tail that overshoots takes real audio off the end with it.
fn ticks_to_frames(ticks: u64, rate: SampleRate) -> Option<u64> {
    let time_base = TimeBase::try_new(1, TICKS_PER_SEC)?;
    super::decode::ticks_to_frames(ticks, time_base, rate, super::decode::Rounding::Down)
}

/// `None` where the file isn't Matroska, states no segment, or ends in a cluster out of reach.
fn read_discards(file: &mut File) -> Option<Vec<Discard>> {
    // First, so any other container costs one read.
    if !is_matroska(file) {
        return None;
    }
    let end = file.metadata().ok()?.len();

    let mut budget = MAX_ELEMENT_HEADERS;
    let segment = find_segment(file, end, &mut budget)?;
    let cluster = last_cluster(file, &segment, &mut budget)?;
    Some(cluster_discards(file, &cluster, &mut budget))
}

fn is_matroska(file: &mut File) -> bool {
    let mut magic = [0u8; MAX_ID_LEN];
    read_at(file, 0, &mut magic).is_some() && u32::from_be_bytes(magic) == EBML_HEADER
}

/// An element's id and the span its payload occupies.
#[derive(Clone, Copy)]
struct Element {
    id: u32,
    payload: u64,
    end: u64,
    /// The element stated no size, so `end` is its parent's rather than its own.
    open_ended: bool,
}

fn find_segment(file: &mut File, end: u64, budget: &mut u32) -> Option<Element> {
    let mut pos = 0;
    while pos < end && *budget > 0 {
        *budget -= 1;
        let element = read_element(file, pos, end)?;
        if element.id == SEGMENT {
            return Some(element);
        }
        pos = element.end;
    }
    None
}

/// The last cluster, where a file's trailing padding sits. `None` where a cluster states no size,
/// as a muxer writing to a pipe leaves them, which a walk stepping by size cannot get past.
fn last_cluster(file: &mut File, segment: &Element, budget: &mut u32) -> Option<Element> {
    let mut last = None;
    let mut pos = segment.payload;
    while pos < segment.end && *budget > 0 {
        *budget -= 1;
        let child = read_element(file, pos, segment.end)?;
        pos = child.end;
        if child.id == CLUSTER {
            if child.open_ended {
                return None;
            }
            last = Some(child);
        }
    }
    last
}

/// The last stating block group per track wins, and a group with no padding is not a statement of
/// zero, leaving what came before standing.
fn cluster_discards(file: &mut File, cluster: &Element, budget: &mut u32) -> Vec<Discard> {
    let mut discards: Vec<Discard> = Vec::new();
    let mut pos = cluster.payload;

    while pos < cluster.end && *budget > 0 {
        *budget -= 1;
        let Some(child) = read_element(file, pos, cluster.end) else {
            break;
        };
        pos = child.end;
        if child.id != BLOCK_GROUP {
            continue;
        }
        let Some(discard) = block_group_discard(file, &child, budget) else {
            continue;
        };
        match discards.iter_mut().find(|held| held.track_num == discard.track_num) {
            Some(held) => held.ticks = discard.ticks,
            None => discards.push(discard),
        }
    }

    discards
}

/// The track and the padding one block group states, both required.
fn block_group_discard(file: &mut File, group: &Element, budget: &mut u32) -> Option<Discard> {
    let mut track_num = None;
    let mut ticks = None;
    let mut pos = group.payload;

    while pos < group.end && *budget > 0 {
        *budget -= 1;
        let Some(child) = read_element(file, pos, group.end) else {
            break;
        };
        pos = child.end;
        match child.id {
            BLOCK => track_num = block_track(file, &child),
            // Negative states padding at the block's *start*, a splice rather than the encoder's
            // tail.
            DISCARD_PADDING => {
                ticks = signed(file, &child).and_then(|stated| u64::try_from(stated).ok());
            }
            _ => {}
        }
    }

    Some(Discard { track_num: track_num?, ticks: ticks.filter(|ticks| *ticks > 0)? })
}

/// The track a block names itself with, its first field being a vint of the same shape as a size.
fn block_track(file: &mut File, block: &Element) -> Option<u32> {
    let mut head = [0u8; MAX_VINT_LEN];
    let stated = usize::try_from(block.end.checked_sub(block.payload)?).unwrap_or(head.len());
    let read = stated.min(head.len());
    if read == 0 {
        return None;
    }
    read_at(file, block.payload, head.get_mut(..read)?)?;
    u32::try_from(vint(head.get(..read)?)?.0?).ok()
}

/// A signed integer of the width the element states, which EBML lets a muxer choose.
fn signed(file: &mut File, element: &Element) -> Option<i64> {
    let width = usize::try_from(element.end.checked_sub(element.payload)?).ok()?;
    if width == 0 || width > MAX_VINT_LEN {
        return None;
    }

    let mut stated = [0u8; MAX_VINT_LEN];
    read_at(file, element.payload, stated.get_mut(..width)?)?;

    // Sign-extended from that width into eight bytes.
    let fill = if *stated.first()? & 0x80 == 0 { 0x00 } else { 0xFF };
    let mut bytes = [fill; MAX_VINT_LEN];
    bytes.get_mut(MAX_VINT_LEN - width..)?.copy_from_slice(stated.get(..width)?);
    Some(i64::from_be_bytes(bytes))
}

/// The id and size at `pos`, kept forward and inside its parent's `end`, so a size of the file's
/// own choosing can neither loop the walk nor send it past the end.
fn read_element(file: &mut File, pos: u64, end: u64) -> Option<Element> {
    let mut head = [0u8; ELEMENT_HEADER_LEN];
    let available = usize::try_from(end.checked_sub(pos)?).unwrap_or(head.len());
    let read = available.min(head.len());
    read_at(file, pos, head.get_mut(..read)?)?;

    let (id, id_len) = element_id(head.get(..read)?)?;
    let (size, size_len) = vint(head.get(id_len..read)?)?;
    let payload = pos.checked_add(u64::try_from(id_len + size_len).ok()?)?;

    // An unknown size runs to the end of the parent; only the cluster's caller acts on it.
    let element_end = match size {
        Some(size) => payload.checked_add(size)?,
        None => end,
    };

    (element_end <= end && payload <= element_end).then_some(Element {
        id,
        payload,
        end: element_end,
        open_ended: size.is_none(),
    })
}

/// An element id and its length, marker bits kept, ids being written with them everywhere.
fn element_id(bytes: &[u8]) -> Option<(u32, usize)> {
    let len = vint_len(bytes.first())?;
    if len > MAX_ID_LEN {
        return None;
    }
    let mut id = [0u8; MAX_ID_LEN];
    id.get_mut(MAX_ID_LEN - len..)?.copy_from_slice(bytes.get(..len)?);
    Some((u32::from_be_bytes(id), len))
}

/// A variable-length integer and its length, marker bits off. The value is `None` where every
/// payload bit is set, the spec's spelling of an unknown size.
fn vint(bytes: &[u8]) -> Option<(Option<u64>, usize)> {
    let len = vint_len(bytes.first())?;
    if len > MAX_VINT_LEN {
        return None;
    }

    let mut value = [0u8; MAX_VINT_LEN];
    value.get_mut(MAX_VINT_LEN - len..)?.copy_from_slice(bytes.get(..len)?);
    // The leading one-bit marks the width: a one-byte vint keeps seven payload bits, an eight-byte
    // one none in its first byte.
    let marker = MAX_VINT_LEN - len;
    *value.get_mut(marker)? &= 0xFFu8.checked_shr(u32::try_from(len).ok()?).unwrap_or(0);

    let stated = u64::from_be_bytes(value);
    let unknown = stated == (1u64 << (7 * u32::try_from(len).ok()?)) - 1;
    Some(((!unknown).then_some(stated), len))
}

/// How many bytes a vint or an id occupies, which its leading one-bit states.
fn vint_len(first: Option<&u8>) -> Option<usize> {
    let leading_zeros = usize::try_from(first?.leading_zeros()).ok()?;
    // An all-zero first byte states a width wider than EBML allows.
    (leading_zeros < MAX_VINT_LEN).then_some(leading_zeros + 1)
}

fn read_at(file: &mut File, pos: u64, buf: &mut [u8]) -> Option<()> {
    file.seek(SeekFrom::Start(pos)).ok()?;
    file.read_exact(buf).ok()
}

#[cfg(test)]
#[path = "tests/mkv_trim_tests.rs"]
mod tests;
