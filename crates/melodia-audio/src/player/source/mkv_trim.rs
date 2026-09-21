//! The trailing padding a Matroska file states, and that nothing between the container and the
//! decoder acts on.
//!
//! `symphonia-format-mkv` parses `DiscardPadding` into a field on its own block group struct, marks
//! that struct `#[allow(dead_code)]`, and reads it nowhere: the block is destructured for its data
//! and its duration, and the packet is built through the constructor that zeroes both trims. So
//! `Packet::trim_end` arrives as zero however the gapless option is set, so a decoder that honours
//! it is told nothing. Ours does, which is why the same file in Ogg comes out exact. Reading the
//! element back is ours to do, the same way [`super::aac_trim`] reads an MP4 edit list the demuxer
//! also keeps to itself.
//!
//! Container-scoped rather than codec-scoped. `DiscardPadding` is Matroska's statement about its own
//! block, so it applies to whatever codec the track carries, and the reader fills neither packet
//! trim for any of them. `CodecDelay` is the half that does work, subtracted from the block
//! timestamps, which is why only the tail leaks.
//!
//! The walk answers in Matroska ticks, which are nanoseconds and are *not* scaled by
//! `TimestampScale`. Converting them needs the decoder's sample rate, so [`resolve`] is the second
//! half and runs once the decoder is built.

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

/// Element headers the walk will read before giving up.
///
/// It reads one per top-level element, one per cluster, and one per element inside the last cluster
/// alone. A cluster holds a few seconds, so the real count is in the hundreds. The budget only
/// bounds a file claiming a million empty elements, which would otherwise cost a seek and a read
/// per two bytes of it.
const MAX_ELEMENT_HEADERS: u32 = 8192;

/// The longest trailing padding worth believing, in seconds.
///
/// The sibling of [`super::aac_trim`]'s ceiling on a head, and it does the same job at the other
/// end: no encoder pads anything near a second, so a larger number is a misread or a muxer using
/// the element for something other than encoder padding, and acting on it would cut real audio off
/// the end. It is also what bounds the window that holds those frames back.
const TAIL_CEILING_SECS: u64 = 1;

/// The trailing padding one track's last stating block declares, in Matroska ticks.
pub(super) struct Discard {
    pub track_num: u32,
    pub ticks: u64,
}

/// What each track's last stating block declares, keyed by the track it belongs to.
///
/// Keyed rather than taken as the file's one answer because an `.mka` may hold more than one track,
/// and a padding stated for a second audio or a subtitle track means nothing to the one being
/// decoded. The ids line up: the reader builds every track from the Matroska `TrackNumber`, which is
/// what a block names itself with.
///
/// The handle is rewound before this returns, the caller handing the same one to the demuxer.
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

/// Converts a tick count into decoded frames, rounded **down**: a tail that overshoots takes real
/// audio off the end with it.
fn ticks_to_frames(ticks: u64, rate: SampleRate) -> Option<u64> {
    let time_base = TimeBase::try_new(1, TICKS_PER_SEC)?;
    super::decode::ticks_to_frames(ticks, time_base, rate, super::decode::Rounding::Down)
}

/// `None` where the file is not Matroska, states no segment, or ends in a cluster this cannot reach.
fn read_discards(file: &mut File) -> Option<Vec<Discard>> {
    let end = file.metadata().ok()?.len();
    // One read, so a file of any other container costs nothing more than this.
    if !is_matroska(file) {
        return None;
    }

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
    /// True where the element stated no size, so `end` is its parent's rather than its own.
    open_ended: bool,
}

/// The segment, which everything below sits inside.
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

/// The last cluster in `segment`, which is where a file's trailing padding sits.
///
/// `None` where any cluster states an unknown size, which a walk stepping over payloads by size
/// cannot get past: a muxer writing to a pipe leaves them that way, and answering out of the cluster
/// before it would read some other block's padding.
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

/// The padding each track's last stating block group declares inside `cluster`.
///
/// A group earlier in the cluster can state one too, so the last for each track wins rather than the
/// first. A group with no padding is not a statement of zero and leaves whatever came before it
/// standing.
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

/// The track and the padding one block group states, where it states both.
///
/// Both halves are required: a padding naming no track cannot be matched against the one being
/// decoded, and a block stating none has nothing to contribute.
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
            // A negative value states padding at the *start* of the block, which is a splice rather
            // than the encoder's tail and not what is left in the samples here.
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

/// Reads the id and size sitting at `pos`, bounded by its parent's `end`.
///
/// The two bounds at the bottom keep the walk moving forward and inside its parent, so a size field
/// of the file's own choosing can neither loop it nor send it reading past the end. A short read is
/// nothing to recover from: where the next element starts is whatever this one's size said.
fn read_element(file: &mut File, pos: u64, end: u64) -> Option<Element> {
    let mut head = [0u8; ELEMENT_HEADER_LEN];
    let available = usize::try_from(end.checked_sub(pos)?).unwrap_or(head.len());
    let read = available.min(head.len());
    read_at(file, pos, head.get_mut(..read)?)?;

    let (id, id_len) = element_id(head.get(..read)?)?;
    let (size, size_len) = vint(head.get(id_len..read)?)?;
    let payload = pos.checked_add(u64::try_from(id_len + size_len).ok()?)?;

    // An unknown size runs to the end of the parent, which is what a muxer writing to a pipe leaves
    // on the segment. Only the cluster's caller acts on the distinction.
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

/// An element id and the bytes it occupied, marker bits kept: ids are written with them everywhere,
/// the constants above included.
fn element_id(bytes: &[u8]) -> Option<(u32, usize)> {
    let len = vint_len(bytes.first())?;
    if len > MAX_ID_LEN {
        return None;
    }
    let mut id = [0u8; MAX_ID_LEN];
    id.get_mut(MAX_ID_LEN - len..)?.copy_from_slice(bytes.get(..len)?);
    Some((u32::from_be_bytes(id), len))
}

/// A variable-length integer and the bytes it occupied, with the marker bits taken off.
///
/// The value is `None` where every payload bit is set, which is the spec's way of stating an unknown
/// size.
fn vint(bytes: &[u8]) -> Option<(Option<u64>, usize)> {
    let len = vint_len(bytes.first())?;
    if len > MAX_VINT_LEN {
        return None;
    }

    let mut value = [0u8; MAX_VINT_LEN];
    value.get_mut(MAX_VINT_LEN - len..)?.copy_from_slice(bytes.get(..len)?);
    // The leading one-bit marks the width and is not part of the value. A one-byte marker leaves
    // seven payload bits, an eight-byte one leaves none in its first byte at all.
    let marker = MAX_VINT_LEN - len;
    *value.get_mut(marker)? &= 0xFFu8.checked_shr(u32::try_from(len).ok()?).unwrap_or(0);

    let stated = u64::from_be_bytes(value);
    let unknown = stated == (1u64 << (7 * u32::try_from(len).ok()?)) - 1;
    Some(((!unknown).then_some(stated), len))
}

/// How many bytes a vint or an id occupies, which its leading one-bit states.
fn vint_len(first: Option<&u8>) -> Option<usize> {
    let leading_zeros = usize::try_from(first?.leading_zeros()).ok()?;
    // An all-zero first byte states a width wider than EBML allows, and would otherwise read as one
    // byte longer than the longest legal vint.
    (leading_zeros < MAX_VINT_LEN).then_some(leading_zeros + 1)
}

fn read_at(file: &mut File, pos: u64, buf: &mut [u8]) -> Option<()> {
    file.seek(SeekFrom::Start(pos)).ok()?;
    file.read_exact(buf).ok()
}
