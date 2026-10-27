//! Getting the audio out of a segment, whichever of the three shapes it arrives in.
//!
//! **For two of them a depacketiser, not a demuxer**: audio-only MPEG-TS carries one ADTS AAC or
//! MPEG audio stream inside transport and PES framing, and packed audio is that stream behind a tag
//! or two. ISO-BMFF passes untouched, a fragment already being its demuxer's framing. Boundaries
//! need no smoothing: ADTS resyncs on every packet, and a fragment splices on a box boundary.

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

/// ADTS AAC and the two MPEG audio layers. LATM (`0x11`) is absent, Symphonia being unable to
/// decode it, so selecting nothing fails saying so where selecting it would fail on its bytes.
const AUDIO_STREAM_TYPES: [u8; 3] = [0x03, 0x04, 0x0F];

const PES_START_CODE: &[u8] = &[0x00, 0x00, 0x01];
/// PES stream ids in this range are audio.
const PES_AUDIO_IDS: std::ops::RangeInclusive<u8> = 0xC0..=0xDF;
/// Prefix, stream id, packet length, two flag bytes, then the header length itself.
const PES_HEADER_LEN_OFFSET: usize = 8;

/// Stateful only to hold the elementary pid, a server sometimes omitting the tables that name it.
#[derive(Default)]
pub struct SegmentReader {
    audio_pid: Option<u16>,
    codec: &'static str,
}

impl SegmentReader {
    /// A segment naming no audio contributes nothing rather than failing: one bad segment in a live
    /// stream is a gap, and the next one usually plays.
    pub fn append(&mut self, segment: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        // Every arm is bounded by the input, and the transport one appends per packet.
        out.reserve(segment.len());
        // ISO-BMFF first, by its box type: three sync bytes 188 apart aren't impossible inside a
        // fragment, and unwrapping one is exactly wrong.
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
            let body = if is_unit_start(packet) { pes_body(payload) } else { Some(payload) };
            if let Some(body) = body {
                out.extend_from_slice(body);
            }
        }
    }
}

/// What a fragmented MP4 segment can open with, `ftyp`/`moov` being the init segment's. The type
/// sits at offset 4, behind the box's length.
const ISO_BMFF_BOXES: [&[u8; 4]; 6] = [b"ftyp", b"styp", b"moof", b"moov", b"sidx", b"emsg"];

fn is_iso_bmff(segment: &[u8]) -> bool {
    segment.get(4..8).is_some_and(|kind| ISO_BMFF_BOXES.iter().any(|known| *known == kind))
}

/// Searched rather than assumed at zero, a few servers padding the front.
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

/// Off the program tables, unreassembled: a station's tables fit one packet, and a server splitting
/// one falls back to the PES scan.
fn find_audio_pid(segment: &[u8]) -> Option<u16> {
    let program_pids = program_map_pids(segment);
    let mut maps = packets(segment)
        .filter(|packet| program_pids.contains(&pid(packet)))
        .filter_map(psi_section)
        .filter_map(|section| section_body(section, TABLE_ID_PMT))
        .peekable();

    // On a map having *arrived*, not on one naming something: the scan is for tables that never
    // came, and overriding a readable map would feed the probe a type it can only fail on.
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

/// The first elementary pid of a stream type we can decode, off the section's body.
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

/// Drops the timestamp tags packed audio opens with, looped since servers write several, and
/// stripped rather than resynced past, a tag's payload able to read as a sync word.
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

/// ADTS first, its sync word also satisfying the looser MPEG audio one.
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
