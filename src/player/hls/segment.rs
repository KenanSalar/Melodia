//! Getting the audio out of a segment, whichever of the three shapes it arrives in.
//!
//! **For two of them this is a depacketiser, not a demuxer.** Audio-only MPEG-TS carries one
//! elementary stream that is already ADTS AAC or MPEG audio, so recovering it is a matter of
//! dropping the transport and PES framing around it, and packed audio is that stream already,
//! behind a tag or two. What comes out is a container Symphonia reads today, which is why HLS
//! needs no demuxer nobody has written. The third, ISO-BMFF, is handed on untouched: a fragment is
//! already the framing its own demuxer wants.
//!
//! Segment boundaries are not smoothed over here and do not need to be. `AdtsReader` scans for its
//! sync word on every packet, so a splice there costs at most the frame it lands in, and a
//! fragment splices on a box boundary, which is where the reader behind it looks for the next one.

/// Transport packets are a fixed 188 bytes and each begins with this byte.
const TS_PACKET_LEN: usize = 188;
const TS_SYNC_BYTE: u8 = 0x47;
/// How many consecutive sync bytes confirm the packet grid. Two would false-positive on audio.
const TS_SYNC_CONFIRMATIONS: usize = 3;

const PAT_PID: u16 = 0x0000;
const TABLE_ID_PAT: u8 = 0x00;
const TABLE_ID_PMT: u8 = 0x02;
/// Table id, then a 12-bit length covering everything after it including the trailing CRC.
const PSI_HEADER_LEN: usize = 8;
const PSI_CRC_LEN: usize = 4;
/// Below this a section cannot hold its own header and CRC, so its length field is nonsense.
const PSI_MIN_LENGTH: usize = PSI_HEADER_LEN - 3 + PSI_CRC_LEN;

/// Stream types worth selecting: ADTS AAC and the two MPEG audio layers.
///
/// LATM (`0x11`) is deliberately absent. Symphonia cannot decode it, and pointing the probe at its
/// pid would feed it bytes it can only fail on, where selecting nothing fails saying so.
const AUDIO_STREAM_TYPES: [u8; 3] = [0x03, 0x04, 0x0F];

const PES_START_CODE: &[u8] = &[0x00, 0x00, 0x01];
/// PES stream ids in this range are audio.
const PES_AUDIO_IDS: std::ops::RangeInclusive<u8> = 0xC0..=0xDF;
/// Prefix, stream id, packet length, two flag bytes, then the header length itself.
const PES_HEADER_LEN_OFFSET: usize = 8;

/// Unwraps segments into one continuous elementary stream.
///
/// Stateful only to hold the elementary pid: the tables naming it repeat in every segment, but a
/// server that omits them from one still has to be read.
#[derive(Default)]
pub struct SegmentReader {
    audio_pid: Option<u16>,
    codec: &'static str,
}

impl SegmentReader {
    /// Append the audio inside `segment` to `out`.
    ///
    /// A segment that names no audio contributes nothing rather than failing: one bad segment in a
    /// live stream is a gap, and the next one usually plays.
    pub fn append(&mut self, segment: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        // Exact: every arm below is bounded above by the input, two copying it whole and the
        // transport one stripping headers. Worth reserving because that arm appends per packet,
        // so a segment lands in hundreds of pushes rather than one.
        out.reserve(segment.len());
        // ISO-BMFF is asked about first, and by its box type rather than by elimination: a
        // fragment is already the framing its demuxer wants, so unwrapping it is exactly wrong,
        // and three sync bytes 188 apart are not impossible inside one.
        if is_iso_bmff(segment) {
            out.extend_from_slice(segment);
        } else {
            match packet_offset(segment) {
                Some(offset) => self.append_transport_stream(&segment[offset..], out),
                None => out.extend_from_slice(strip_id3(segment)),
            }
        }
        if self.codec.is_empty() {
            self.codec = codec_of(&out[start..]);
        }
    }

    /// `AAC` or `MP3` in the directory's own spelling, empty until a segment has been read.
    pub fn codec(&self) -> &'static str {
        self.codec
    }

    fn append_transport_stream(&mut self, segment: &[u8], out: &mut Vec<u8>) {
        if self.audio_pid.is_none() {
            self.audio_pid = find_audio_pid(segment);
        }
        let Some(audio_pid) = self.audio_pid else {
            return;
        };

        for packet in packets(segment) {
            if pid(packet) != audio_pid {
                continue;
            }
            let Some(payload) = payload(packet) else {
                continue;
            };
            let body = if is_unit_start(packet) {
                pes_body(payload)
            } else {
                Some(payload)
            };
            if let Some(body) = body {
                out.extend_from_slice(body);
            }
        }
    }
}

/// The box types a segment of a fragmented MP4 stream can open with.
///
/// `ftyp`/`moov` is the init segment; the media segments open with one of the rest. The type sits
/// at offset 4, behind the box's own length.
const ISO_BMFF_BOXES: [&[u8; 4]; 6] = [b"ftyp", b"styp", b"moof", b"moov", b"sidx", b"emsg"];

fn is_iso_bmff(segment: &[u8]) -> bool {
    segment.get(4..8).is_some_and(|kind| ISO_BMFF_BOXES.iter().any(|known| *known == kind))
}

/// Where the packet grid starts, or `None` for a segment that is not transport-stream framed.
///
/// Searched rather than assumed at zero: a few servers pad the front, and every misaligned offset
/// reads as plausible garbage.
fn packet_offset(segment: &[u8]) -> Option<usize> {
    (0..TS_PACKET_LEN).find(|offset| {
        (0..TS_SYNC_CONFIRMATIONS)
            .all(|n| segment.get(offset + n * TS_PACKET_LEN) == Some(&TS_SYNC_BYTE))
    })
}

fn packets(segment: &[u8]) -> impl Iterator<Item = &[u8]> {
    segment.chunks_exact(TS_PACKET_LEN).filter(|packet| packet.first() == Some(&TS_SYNC_BYTE))
}

fn pid(packet: &[u8]) -> u16 {
    let high = u16::from(packet.get(1).copied().unwrap_or(0) & 0x1F);
    (high << 8) | u16::from(packet.get(2).copied().unwrap_or(0))
}

fn is_unit_start(packet: &[u8]) -> bool {
    packet.get(1).is_some_and(|byte| byte & 0x40 != 0)
}

/// The packet's payload, past the adaptation field it may carry first.
fn payload(packet: &[u8]) -> Option<&[u8]> {
    let start = match (packet.get(3)? >> 4) & 0b11 {
        1 => 4,
        3 => 5 + usize::from(*packet.get(4)?),
        // 0 is reserved and 2 is an adaptation field filling the whole packet.
        _ => return None,
    };
    packet.get(start..).filter(|payload| !payload.is_empty())
}

/// A program-information section, past the pointer that says where in the payload it starts.
fn psi_section(packet: &[u8]) -> Option<&[u8]> {
    if !is_unit_start(packet) {
        return None;
    }
    let payload = payload(packet)?;
    payload.get(1 + usize::from(*payload.first()?)..)
}

/// The elementary pid carrying audio, read off the program tables.
///
/// Sections spanning more than one packet are not reassembled: a radio station's tables are a few
/// dozen bytes and fit with room to spare, and a server that split one falls back to the PES scan.
fn find_audio_pid(segment: &[u8]) -> Option<u16> {
    let program_pids = program_map_pids(segment);
    let mut maps = packets(segment)
        .filter(|packet| program_pids.contains(&pid(packet)))
        .filter_map(psi_section)
        .filter_map(|section| section_body(section, TABLE_ID_PMT))
        .peekable();

    // Keyed on a map having *arrived*, not on one having named something: the scan is for tables
    // that never came, and letting it override a readable map is how a stream whose only audio is
    // a type [`AUDIO_STREAM_TYPES`] leaves out reaches a probe that can only fail on it.
    if maps.peek().is_none() {
        return first_audio_pes_pid(segment);
    }
    maps.find_map(audio_pid_from_pmt)
}

fn program_map_pids(segment: &[u8]) -> Vec<u16> {
    packets(segment)
        .filter(|packet| pid(packet) == PAT_PID)
        .filter_map(psi_section)
        .find_map(|section| section_body(section, TABLE_ID_PAT))
        .map(|body| {
            body.chunks_exact(4)
                // Program 0 names the network information table, not a program map.
                .filter(|entry| u16::from_be_bytes([entry[0], entry[1]]) != 0)
                .map(|entry| u16::from_be_bytes([entry[2], entry[3]]) & 0x1FFF)
                .collect()
        })
        .unwrap_or_default()
}

/// The first elementary pid a program map names with a stream type we have a decoder for. Takes
/// the section's body, [`find_audio_pid`] having read the header off to recognise the table at all.
fn audio_pid_from_pmt(body: &[u8]) -> Option<u16> {
    // Clock reference pid, then a descriptor block sized by its own 12-bit length.
    let program_info_len = (usize::from(body.get(2)? & 0x0F) << 8) | usize::from(*body.get(3)?);
    let mut cursor = 4 + program_info_len;

    while let Some(entry) = body.get(cursor..cursor + 5) {
        let stream_type = entry[0];
        let elementary_pid = u16::from_be_bytes([entry[1], entry[2]]) & 0x1FFF;
        let info_len = (usize::from(entry[3] & 0x0F) << 8) | usize::from(entry[4]);
        if AUDIO_STREAM_TYPES.contains(&stream_type) {
            return Some(elementary_pid);
        }
        cursor += 5 + info_len;
    }
    None
}

/// The section's payload, between its fixed header and its CRC.
fn section_body(section: &[u8], table_id: u8) -> Option<&[u8]> {
    if *section.first()? != table_id {
        return None;
    }
    let length = (usize::from(section.get(1)? & 0x0F) << 8) | usize::from(*section.get(2)?);
    if length < PSI_MIN_LENGTH {
        return None;
    }
    let end = 3 + length;
    if end > section.len() {
        return None;
    }
    section.get(PSI_HEADER_LEN..end - PSI_CRC_LEN)
}

/// The first pid seen carrying an audio PES, for a segment whose tables never arrived.
fn first_audio_pes_pid(segment: &[u8]) -> Option<u16> {
    packets(segment)
        .filter(|packet| is_unit_start(packet))
        .find(|packet| payload(packet).and_then(pes_body).is_some())
        .map(pid)
}

/// The payload of an audio PES packet, past its header.
fn pes_body(payload: &[u8]) -> Option<&[u8]> {
    if payload.get(..PES_START_CODE.len())? != PES_START_CODE {
        return None;
    }
    if !PES_AUDIO_IDS.contains(payload.get(PES_START_CODE.len())?) {
        return None;
    }
    let header_len = usize::from(*payload.get(PES_HEADER_LEN_OFFSET)?);
    payload.get(PES_HEADER_LEN_OFFSET + 1 + header_len..)
}

/// Drop the timestamp tags a packed-audio segment is required to open with.
///
/// Looped because servers write more than one, and stripped rather than left to the demuxer's own
/// resync: a tag's payload can carry bytes that read as a sync word.
fn strip_id3(mut segment: &[u8]) -> &[u8] {
    while let Some(len) = id3_len(segment) {
        match segment.get(len..) {
            Some(rest) => segment = rest,
            None => return &[],
        }
    }
    segment
}

fn id3_len(segment: &[u8]) -> Option<usize> {
    if segment.get(..3)? != b"ID3" {
        return None;
    }
    let has_footer = segment.get(5)? & 0x10 != 0;
    // Four sync-safe bytes: seven bits each, so no byte can look like a frame sync.
    let size = segment
        .get(6..10)?
        .iter()
        .fold(0usize, |size, byte| (size << 7) | usize::from(byte & 0x7F));
    Some(10 + size + if has_footer { 10 } else { 0 })
}

/// What the elementary stream turned out to be, in the spelling the directory uses.
///
/// ADTS is checked first because its sync word also satisfies the looser MPEG audio one.
fn codec_of(stream: &[u8]) -> &'static str {
    match stream {
        [0xFF, second, ..] if second & 0xF6 == 0xF0 => "AAC",
        [0xFF, second, ..] if second & 0xE0 == 0xE0 => "MP3",
        _ => "",
    }
}

#[cfg(test)]
#[path = "tests/segment_tests.rs"]
mod tests;
