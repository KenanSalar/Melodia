//! Unwrapping a segment, over fixtures built the way a station builds them.
//!
//! Transport streams are the half worth building by hand: every field here is a bit-packed offset
//! into a fixed 188-byte grid, and every way of reading one wrong yields bytes that look like
//! audio and decode into nothing. A station serving one is the only place that shows up.

use super::*;

const PMT_PID: u16 = 0x1000;
const AUDIO_PID: u16 = 0x0101;
/// What one packet's payload holds, past the four-byte transport header.
const TS_PAYLOAD_LEN: usize = TS_PACKET_LEN - 4;
/// Adaptation-field stuffing on the continuation packet, which is where a real stream puts its
/// clock reference and the reason the payload does not always start at byte four.
const ADAPTATION_STUFFING: usize = 7;

fn pid_bytes(pid: u16) -> (u8, u8) {
    (u8::try_from(pid >> 8).unwrap_or(0) & 0x1F, u8::try_from(pid & 0xFF).unwrap_or(0))
}

/// One 188-byte packet carrying `payload` and nothing else, stuffed out to length.
fn ts_packet(pid: u16, unit_start: bool, payload: &[u8]) -> Vec<u8> {
    let (high, low) = pid_bytes(pid);
    let mut packet = vec![
        TS_SYNC_BYTE,
        if unit_start { 0x40 | high } else { high },
        low,
        // Payload only, continuity counter zero.
        0x10,
    ];
    packet.extend_from_slice(payload);
    packet.resize(TS_PACKET_LEN, 0xFF);
    packet
}

/// The same, with an adaptation field in front of the payload.
fn ts_packet_with_adaptation(pid: u16, payload: &[u8]) -> Vec<u8> {
    let (high, low) = pid_bytes(pid);
    let mut packet = vec![TS_SYNC_BYTE, high, low, 0x30];
    packet.push(u8::try_from(ADAPTATION_STUFFING).unwrap_or(0));
    packet.resize(5 + ADAPTATION_STUFFING, 0xFF);
    packet.extend_from_slice(payload);
    packet.resize(TS_PACKET_LEN, 0xFF);
    packet
}

/// A unit-start PSI payload opens with a pointer saying where in it the section begins.
fn psi_payload(section: &[u8]) -> Vec<u8> {
    let mut payload = vec![0x00];
    payload.extend_from_slice(section);
    payload
}

/// A program-information section: the eight-byte header the reader skips, the body it reads, and
/// a CRC it never checks.
fn psi_section(table_id: u8, body: &[u8]) -> Vec<u8> {
    // The length field counts everything after it, which is the last five header bytes.
    let length = 5 + body.len() + PSI_CRC_LEN;
    let mut section = vec![
        table_id,
        0xB0 | u8::try_from(length >> 8).unwrap_or(0),
        u8::try_from(length & 0xFF).unwrap_or(0),
        0x00,
        0x01,
        0xC1,
        0x00,
        0x00,
    ];
    section.extend_from_slice(body);
    section.extend_from_slice(&[0; PSI_CRC_LEN]);
    section
}

/// A pid inside a table entry, where the three bits above it are reserved and set.
fn table_pid_bytes(pid: u16) -> (u8, u8) {
    let (high, low) = pid_bytes(pid);
    (0xE0 | high, low)
}

fn pat_section() -> Vec<u8> {
    let (high, low) = table_pid_bytes(PMT_PID);
    psi_section(TABLE_ID_PAT, &[0x00, 0x01, high, low])
}

fn pmt_section(stream_type: u8) -> Vec<u8> {
    let (high, low) = table_pid_bytes(AUDIO_PID);
    // Clock-reference pid, then an empty descriptor block, then one elementary stream with an
    // empty one of its own.
    let body = [0xE1, 0x00, 0xF0, 0x00, stream_type, high, low, 0xF0, 0x00];
    psi_section(TABLE_ID_PMT, &body)
}

/// Prefix, audio stream id, length, two flag bytes and a five-byte header the body sits behind.
const PES_PREFIX_LEN: usize = 14;

fn pes_packet(elementary: &[u8]) -> Vec<u8> {
    let mut pes = vec![0x00, 0x00, 0x01, 0xC0, 0x00, 0x00, 0x80, 0x80, 0x05];
    pes.resize(PES_PREFIX_LEN, 0x00);
    pes.extend_from_slice(elementary);
    pes
}

/// As much elementary stream as fills one packet exactly, so nothing under test has to tell
/// stuffing from audio.
fn adts_frame(len: usize) -> Vec<u8> {
    let mut frame = vec![0xFF, 0xF1, 0x50, 0x80, 0x00, 0x1F, 0xFC];
    frame.resize(len, 0xA5);
    frame
}

fn first_frame() -> Vec<u8> {
    adts_frame(TS_PAYLOAD_LEN - PES_PREFIX_LEN)
}

/// A three-packet segment: the tables naming the audio pid, then the audio.
fn transport_segment(elementary: &[u8]) -> Vec<u8> {
    let mut segment = ts_packet(PAT_PID, true, &psi_payload(&pat_section()));
    segment.extend(ts_packet(PMT_PID, true, &psi_payload(&pmt_section(0x0F))));
    segment.extend(ts_packet(AUDIO_PID, true, &pes_packet(elementary)));
    segment
}

fn unwrapped(segment: &[u8]) -> (Vec<u8>, &'static str) {
    let mut reader = SegmentReader::default();
    let mut out = Vec::new();
    reader.append(segment, &mut out);
    (out, reader.codec())
}

/// The whole path, end to end: the tables name the elementary pid, the PES header comes off, and
/// what is left is a container Symphonia already reads.
#[test]
fn a_transport_segment_comes_out_as_the_elementary_stream_inside_it() {
    let elementary = first_frame();
    let (out, codec) = unwrapped(&transport_segment(&elementary));

    assert_eq!(out, elementary);
    assert_eq!(codec, "AAC");
}

/// A PES spans as many packets as it needs, and only the first carries a header to strip. The
/// continuation also carries an adaptation field, which is what moves its payload off byte four.
#[test]
fn a_continuation_packet_is_appended_whole_and_past_its_adaptation_field() {
    let elementary = first_frame();
    let continued = vec![0xC3; TS_PAYLOAD_LEN - 1 - ADAPTATION_STUFFING];

    let mut segment = transport_segment(&elementary);
    segment.extend(ts_packet_with_adaptation(AUDIO_PID, &continued));

    let (out, _) = unwrapped(&segment);
    assert_eq!(out, [elementary, continued].concat());
}

/// The grid is searched for rather than assumed at zero, a few servers padding the front — and
/// every misaligned offset reads as plausible garbage rather than as an error.
#[test]
fn the_packet_grid_is_searched_for_rather_than_assumed_at_zero() {
    let elementary = first_frame();
    let mut padded = vec![0x00; 5];
    padded.extend(transport_segment(&elementary));

    assert_eq!(packet_offset(&padded), Some(5));
    assert_eq!(packet_offset(&elementary), None, "packed audio is not transport-stream framed");

    let (out, _) = unwrapped(&padded);
    assert_eq!(out, elementary);
}

/// A server that omits the tables from one segment still has to be read, so the elementary pid
/// falls back to the first one seen carrying an audio PES.
#[test]
fn a_segment_with_no_tables_finds_its_audio_by_the_pes_header() {
    let elementary = first_frame();
    let packet = ts_packet(AUDIO_PID, true, &pes_packet(&elementary));
    let segment = [packet.clone(), packet.clone(), packet].concat();

    let (out, codec) = unwrapped(&segment);
    assert_eq!(out, [&elementary[..], &elementary[..], &elementary[..]].concat());
    assert_eq!(codec, "AAC");
}

/// LATM is deliberately not selected: Symphonia cannot decode it, so pointing the probe at its pid
/// feeds it bytes it can only fail on, where selecting nothing fails saying so.
///
/// Which is the whole reason the PES scan is keyed on a map having arrived rather than on one
/// having named something. Its packets are ordinary audio PES, so a scan reached from here takes
/// exactly the pid the map just refused.
#[test]
fn a_stream_type_with_no_decoder_behind_it_is_not_selected() {
    let mut segment = ts_packet(PAT_PID, true, &psi_payload(&pat_section()));
    segment.extend(ts_packet(PMT_PID, true, &psi_payload(&pmt_section(0x11))));
    segment.extend(ts_packet(AUDIO_PID, true, &pes_packet(&first_frame())));

    let (out, codec) = unwrapped(&segment);
    assert!(out.is_empty(), "a LATM stream was selected and handed to a decoder that fails on it");
    assert!(codec.is_empty());
}

/// A fragment is already the framing its own demuxer wants, so unwrapping one is exactly wrong.
#[test]
fn an_iso_bmff_fragment_is_handed_on_untouched() {
    for kind in [b"ftyp", b"styp", b"moof", b"moov", b"sidx", b"emsg"] {
        let mut fragment = vec![0x00, 0x00, 0x01, 0x00];
        fragment.extend_from_slice(kind);
        fragment.resize(512, 0x5A);

        assert!(is_iso_bmff(&fragment), "a box opening with `{kind:?}` was not recognised");
        let (out, _) = unwrapped(&fragment);
        assert_eq!(out, fragment);
    }

    assert!(!is_iso_bmff(&transport_segment(&first_frame())));
    assert!(!is_iso_bmff(&[0x00, 0x00, 0x01, 0x00, b'f', b't', b'y']), "a box type is four bytes");
}

/// Which of the two questions is asked first, and why it is this one.
///
/// Three sync bytes 188 apart are not impossible inside a fragment, and by elimination that
/// fragment would be run through the transport unwrap and come out as rubbish. Asking about the
/// box type first is what makes the accident unreachable.
#[test]
fn a_fragment_holding_an_accidental_packet_grid_is_still_handed_on_whole() {
    let mut fragment = vec![0x00, 0x00, 0x02, 0x00];
    fragment.extend_from_slice(b"styp");
    fragment.resize(600, 0x5A);
    for offset in [10, 10 + TS_PACKET_LEN, 10 + 2 * TS_PACKET_LEN] {
        fragment[offset] = TS_SYNC_BYTE;
    }

    assert_eq!(packet_offset(&fragment), Some(10), "the fixture no longer holds the accident");
    let (out, _) = unwrapped(&fragment);
    assert_eq!(out, fragment);
}

/// Packed audio opens with the timestamp tags the spec requires of it, and a tag's payload can
/// carry bytes that read as a sync word — so they come off here rather than being left to the
/// demuxer's own resync.
#[test]
fn every_leading_id3_tag_comes_off_a_packed_audio_segment() {
    let audio = adts_frame(64);

    let plain = id3_tag(6, false);
    let with_footer = id3_tag(6, true);
    assert_eq!(strip_id3(&[plain.clone(), audio.clone()].concat()), audio);
    assert_eq!(
        strip_id3(&[plain.clone(), with_footer, plain, audio.clone()].concat()),
        audio,
        "servers write more than one, and a footer is ten bytes the size field does not count"
    );
    assert_eq!(strip_id3(&audio), audio);

    // A tag claiming more than arrived leaves nothing rather than a stream starting mid-tag.
    assert!(strip_id3(&id3_tag(6, false)[..12]).is_empty());

    let (out, codec) = unwrapped(&[id3_tag(6, false), audio.clone()].concat());
    assert_eq!(out, audio);
    assert_eq!(codec, "AAC");
}

/// An `ID3v2` tag: ten header bytes, a payload, and a footer where the flags ask for one.
fn id3_tag(payload_len: usize, footer: bool) -> Vec<u8> {
    let mut tag = vec![b'I', b'D', b'3', 4, 0, if footer { 0x10 } else { 0x00 }];
    // Seven bits a byte, which is what stops a size ever looking like a frame sync.
    for shift in [21_u32, 14, 7, 0] {
        tag.push(u8::try_from((payload_len >> shift) & 0x7F).unwrap_or(0));
    }
    tag.resize(10 + payload_len, 0x00);
    if footer {
        tag.resize(20 + payload_len, 0x00);
    }
    tag
}

/// ADTS is asked about first because its sync word also satisfies the looser MPEG audio one, so
/// the other order calls every AAC stream MP3 and hands it the wrong name in the station's row.
#[test]
fn the_codec_is_read_off_the_sync_word_with_adts_asked_about_first() {
    // All four ADTS sync words, MPEG-2 and MPEG-4, with and without a CRC.
    for second in [0xF0, 0xF1, 0xF8, 0xF9] {
        assert_eq!(codec_of(&[0xFF, second, 0x50, 0x80]), "AAC", "{second:#04X} is ADTS");
    }
    assert_eq!(codec_of(&[0xFF, 0xFB, 0x90, 0x00]), "MP3");
    assert_eq!(codec_of(&[0xFF, 0xE3, 0x18, 0xC4]), "MP3", "MPEG-2.5 layer III still syncs");
    assert_eq!(codec_of(&[0x00, 0x00, 0x00, 0x18]), "");
    assert_eq!(codec_of(&[]), "");
}

/// A segment that names no audio contributes nothing rather than failing: one bad segment in a
/// live stream is a gap, and the next one usually plays.
#[test]
fn a_segment_naming_no_audio_is_a_gap_and_not_an_error() {
    let mut reader = SegmentReader::default();
    let mut out = Vec::new();
    let tables_only = [
        ts_packet(PAT_PID, true, &psi_payload(&pat_section())),
        ts_packet(PMT_PID, true, &psi_payload(&pmt_section(0x0F))),
        ts_packet(PMT_PID, true, &psi_payload(&pmt_section(0x0F))),
    ]
    .concat();

    reader.append(&tables_only, &mut out);
    assert!(out.is_empty());
    assert!(reader.codec().is_empty());

    // And the pid it learned there is what carries the next segment, whose own tables never
    // arrive. Read fresh, the PES scan would have taken the decoy that comes first.
    let elementary = first_frame();
    let decoy = vec![0x11; TS_PAYLOAD_LEN - PES_PREFIX_LEN];
    let untabled = [
        ts_packet(0x0200, true, &pes_packet(&decoy)),
        ts_packet(AUDIO_PID, true, &pes_packet(&elementary)),
        ts_packet(AUDIO_PID, true, &pes_packet(&elementary)),
    ]
    .concat();

    reader.append(&untabled, &mut out);
    assert_eq!(out, [&elementary[..], &elementary[..]].concat());
    assert_eq!(reader.codec(), "AAC");
}
