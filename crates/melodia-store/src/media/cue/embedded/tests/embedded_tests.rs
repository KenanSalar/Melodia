//! A sheet inside a FLAC: the native CUESHEET block's bytes as a sheet, the `CUE_TRACKnn_*`
//! comments laid over it, the walk over the metadata blocks that finds either, and the rows the
//! sheet carves. The FLACs are built byte by byte to RFC 9639, with no audio behind the metadata.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::database::queries::fixtures::make_test_metadata;
use crate::media::cue::parse::rem;
use melodia_core::entities::cue::CueFrames;
use melodia_core::error::AppError;

const RATE: u32 = 44_100;

/// One track of a native block: where it starts in samples, and its index points from there.
struct NativeTrack {
    offset: u64,
    number: u8,
    isrc: &'static str,
    is_audio: bool,
    indexes: Vec<(u64, u8)>,
}

fn audio_track(number: u8, offset: u64) -> NativeTrack {
    NativeTrack { offset, number, isrc: "", is_audio: true, indexes: vec![(0, 1)] }
}

/// The CD's lead-out, numbered 170, which marks where the audio ends.
fn lead_out(offset: u64) -> NativeTrack {
    NativeTrack { offset, number: 170, isrc: "", is_audio: true, indexes: Vec::new() }
}

/// `text` NUL-padded to `width`, as the block's fixed-width fields are.
fn padded(text: &str, width: usize) -> Vec<u8> {
    let mut field = text.as_bytes().to_vec();
    field.resize(width, 0);
    field
}

/// A CUESHEET block's body, RFC 9639 §8.7.
fn native_block(catalog: &str, tracks: &[NativeTrack]) -> Vec<u8> {
    let mut bytes = padded(catalog, 128);
    bytes.extend(0_u64.to_be_bytes());
    bytes.push(0x80);
    bytes.extend([0; 258]);
    bytes.push(u8::try_from(tracks.len()).unwrap_or(u8::MAX));
    for track in tracks {
        bytes.extend(track.offset.to_be_bytes());
        bytes.push(track.number);
        bytes.extend(padded(track.isrc, 12));
        bytes.push(if track.is_audio { 0 } else { 0x80 });
        bytes.extend([0; 13]);
        bytes.push(u8::try_from(track.indexes.len()).unwrap_or(u8::MAX));
        for &(offset, number) in &track.indexes {
            bytes.extend(offset.to_be_bytes());
            bytes.push(number);
            bytes.extend([0; 3]);
        }
    }
    bytes
}

/// Two tracks and a lead-out: track 2 starts four minutes in, with two seconds of pregap.
fn two_track_block() -> Vec<u8> {
    let second = NativeTrack {
        offset: 10_584_000,
        number: 2,
        isrc: "USRC17607839",
        is_audio: true,
        indexes: vec![(0, 0), (88_200, 1)],
    };
    native_block("0724384960650", &[audio_track(1, 0), second, lead_out(26_460_000)])
}

const STREAMINFO: u8 = 0;
const VORBIS_COMMENT: u8 = 4;
const CUESHEET: u8 = 5;

/// STREAMINFO stating `rate`, its 20 bits from byte 10.
fn streaminfo(rate: u32) -> Vec<u8> {
    let mut body = vec![0; 34];
    body[10] = (rate >> 12).to_be_bytes()[3];
    body[11] = (rate >> 4).to_be_bytes()[3];
    body[12] = (rate << 4).to_be_bytes()[3] | 0x02;
    body
}

/// A `VORBIS_COMMENT` body, each comment spelled `KEY=value`.
fn comments(comments: &[&str]) -> Vec<u8> {
    let vendor = b"melodia tests";
    let mut body = u32::try_from(vendor.len()).unwrap_or(0).to_le_bytes().to_vec();
    body.extend(vendor);
    body.extend(u32::try_from(comments.len()).unwrap_or(0).to_le_bytes());
    for comment in comments {
        body.extend(u32::try_from(comment.len()).unwrap_or(0).to_le_bytes());
        body.extend(comment.as_bytes());
    }
    body
}

fn flac(prefix: &[u8], blocks: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    bytes.extend(b"fLaC");
    for (position, (kind, body)) in blocks.iter().enumerate() {
        let last = if position + 1 == blocks.len() { 0x80 } else { 0 };
        bytes.push(last | kind);
        bytes.extend(&u32::try_from(body.len()).unwrap_or(0).to_be_bytes()[1..]);
        bytes.extend(body);
    }
    bytes
}

fn written(tmp: &TempDir, bytes: &[u8]) -> Result<std::path::PathBuf, AppError> {
    let path = tmp.path().join("album.flac");
    fs::write(&path, bytes)?;
    Ok(path)
}

fn frames(count: u32) -> Option<CueFrames> {
    CueFrames::try_from(i64::from(count)).ok()
}

fn cue_track(number: i64) -> Option<CueTrack> {
    CueTrack::try_from(number).ok()
}

#[test]
fn a_native_block_states_its_tracks_and_drops_the_lead_out() {
    let sheet = block::sheet(RATE, &two_track_block());

    let numbers: Option<Vec<u8>> =
        sheet.map(|sheet| sheet.tracks.iter().map(|track| track.number.number()).collect());

    assert_eq!(numbers, Some(vec![1, 2]));
}

/// At 44.1 kHz a cue frame is 588 samples.
#[test]
fn a_native_block_s_index_points_are_its_track_s_offset_plus_their_own() {
    let sheet = block::sheet(RATE, &two_track_block());
    let second = sheet.as_ref().and_then(|sheet| sheet.track(cue_track(2)?));

    let points = second.map(|track| (track.index(0).map(|i| i.at), track.index(1).map(|i| i.at)));

    assert_eq!(points, Some((frames(18_000), frames(18_150))));
}

/// Half a frame is 294 samples at 44.1 kHz: one short of it rounds down, it rounds up.
#[test]
fn a_native_offset_rounds_to_the_nearest_cue_frame() {
    let rows = [(10_584_293, 18_000), (10_584_294, 18_001), (10_584_588, 18_001)];
    for (offset, at) in rows {
        let block = native_block("", &[audio_track(1, offset)]);
        let start = block::sheet(RATE, &block)
            .and_then(|sheet| sheet.tracks.first().and_then(|track| track.index(1)))
            .map(|index| index.at);
        assert_eq!(start, frames(at), "{offset} samples");
    }
}

#[test]
fn a_native_block_s_catalog_and_isrcs_are_read_and_blanks_skipped() {
    let sheet = block::sheet(RATE, &two_track_block());
    let isrcs = |sheet: &Sheet| sheet.tracks.iter().map(|t| t.isrc.clone()).collect::<Vec<_>>();

    let read = sheet.map(|sheet| (sheet.catalog.clone(), isrcs(&sheet)));

    assert_eq!(
        read,
        Some((Some("0724384960650".to_owned()), vec![None, Some("USRC17607839".to_owned())]))
    );
}

#[test]
fn a_native_track_flagged_non_audio_is_kept_as_data() {
    let data = NativeTrack { is_audio: false, ..audio_track(2, 10_584_000) };
    let sheet = block::sheet(RATE, &native_block("", &[audio_track(1, 0), data]));

    let audio: Option<Vec<bool>> = sheet.map(|s| s.tracks.iter().map(|t| t.is_audio).collect());

    assert_eq!(audio, Some(vec![true, false]));
}

#[test]
fn a_native_block_cut_short_does_not_parse() {
    let whole = two_track_block();
    for length in [0, 127, 396, whole.len() - 1] {
        assert!(block::sheet(RATE, &whole[..length]).is_none(), "{length} bytes");
    }
}

#[test]
fn an_override_key_names_its_track_and_field_in_any_case() {
    let rows = [
        ("CUE_TRACK01_TITLE", Some((1, TrackField::Title))),
        ("cue_track1_title", Some((1, TrackField::Title))),
        ("CUE_TRACK12_ARTIST", Some((12, TrackField::Artist))),
        ("CUE_TRACK03_COMPOSER", Some((3, TrackField::Composer))),
        ("CUE_TRACK03_GENRE", Some((3, TrackField::Genre))),
        ("CUE_TRACK03_DATE", Some((3, TrackField::Date))),
        ("CUE_TRACK03_ISRC", Some((3, TrackField::Isrc))),
        ("CUE_TRACK03_LYRICS", None),
        ("CUE_TRACK100_TITLE", None),
        ("CUE_TRACKXX_TITLE", None),
        ("CUE_TRACK01TITLE", None),
        ("CUESHEET", None),
        ("TITLE", None),
        ("", None),
    ];
    for (key, expected) in rows {
        let found = overrides::override_key(key).map(|(track, field)| (track.number(), field));
        assert_eq!(found, expected, "{key:?}");
    }
}

fn overridden(overrides: &[(&str, &str)]) -> Option<Sheet> {
    let mut sheet = block::sheet(RATE, &two_track_block())?;
    let overrides: Vec<(String, String)> =
        overrides.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect();
    overrides::apply(&mut sheet, &overrides);
    Some(sheet)
}

#[test]
fn an_override_sets_its_field_on_its_track() {
    let sheet = overridden(&[
        ("CUE_TRACK02_TITLE", "Two"),
        ("CUE_TRACK02_ARTIST", "Guest"),
        ("CUE_TRACK02_GENRE", "Jazz"),
        ("CUE_TRACK02_ISRC", "GBAYE0601498"),
    ]);
    let second = sheet.as_ref().and_then(|sheet| sheet.track(cue_track(2)?));

    let fields = second.map(|track| {
        let fields = &track.fields;
        (
            fields.title.as_deref(),
            fields.performer.as_deref(),
            fields.rem(rem::GENRE),
            track.isrc.as_deref(),
        )
    });

    assert_eq!(fields, Some((Some("Two"), Some("Guest"), Some("Jazz"), Some("GBAYE0601498"))));
}

#[test]
fn a_blank_override_or_one_numbering_no_track_changes_nothing() {
    let sheet = overridden(&[("CUE_TRACK01_TITLE", "   "), ("CUE_TRACK05_TITLE", "Five")]);

    let titles: Option<Vec<Option<String>>> =
        sheet.map(|sheet| sheet.tracks.iter().map(|track| track.fields.title.clone()).collect());

    assert_eq!(titles, Some(vec![None, None]));
}

#[test]
fn a_flac_carries_a_sheet_in_a_native_block_or_a_comment_in_any_case() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let rows = [
        ("a native block", vec![(CUESHEET, two_track_block())]),
        ("a comment", vec![(VORBIS_COMMENT, comments(&["TITLE=x", "CUESHEET=FILE"]))]),
        ("a lower-case comment", vec![(VORBIS_COMMENT, comments(&["cuesheet=FILE"]))]),
        (
            "a block past the comments",
            vec![(VORBIS_COMMENT, comments(&["A=b"])), (CUESHEET, two_track_block())],
        ),
    ];
    for (label, blocks) in rows {
        let blocks = [vec![(STREAMINFO, streaminfo(RATE))], blocks].concat();
        let path = written(&tmp, &flac(b"", &blocks))?;
        assert!(carries_sheet(&path)?, "{label}");
    }
    Ok(())
}

/// A comment shorter than the key, and one whose value merely mentions it, are read past.
#[test]
fn a_flac_with_neither_carries_no_sheet() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let blocks = [
        (STREAMINFO, streaminfo(RATE)),
        (VORBIS_COMMENT, comments(&["A=b", "COMMENT=CUESHEET=no", "CUESHEETS=no"])),
    ];
    let path = written(&tmp, &flac(b"", &blocks))?;

    assert!(!carries_sheet(&path)?);
    Ok(())
}

/// A tagger can put an `ID3v2` tag in front of `fLaC`, footer and all.
#[test]
fn a_flac_behind_an_id3v2_tag_is_walked_past_it() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let blocks = [(STREAMINFO, streaminfo(RATE)), (CUESHEET, two_track_block())];
    let rows = [
        ("no footer", [b"ID3\x04\x00\x00\x00\x00\x01\x48".as_slice(), &[0; 200]].concat()),
        ("a footer", [b"ID3\x04\x00\x10\x00\x00\x00\x14".as_slice(), &[0; 20], &[0; 10]].concat()),
    ];
    for (label, id3) in rows {
        let path = written(&tmp, &flac(&id3, &blocks))?;
        assert!(carries_sheet(&path)?, "{label}");
    }
    Ok(())
}

#[test]
fn a_file_that_is_no_flac_or_ends_mid_chain_does_not_read() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let whole = flac(b"", &[(STREAMINFO, streaminfo(RATE)), (VORBIS_COMMENT, comments(&["A=b"]))]);
    let rows = [
        ("an Ogg stream", b"OggS\x00\x02".to_vec()),
        ("a cut chain", whole[..whole.len() - 3].to_vec()),
    ];
    for (label, bytes) in rows {
        let path = written(&tmp, &bytes)?;
        assert!(carries_sheet(&path).is_err(), "{label}");
    }
    Ok(())
}

#[test]
fn the_native_block_is_read_with_the_stream_s_rate() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let block = two_track_block();
    let path =
        written(&tmp, &flac(b"", &[(STREAMINFO, streaminfo(48_000)), (CUESHEET, block.clone())]))?;

    let read = chain::read_block(&path)?;

    assert_eq!(read, Some(EmbeddedSource::Block { sample_rate: 48_000, bytes: block }));
    Ok(())
}

#[test]
fn a_flac_with_no_native_block_reads_none() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path = written(
        &tmp,
        &flac(b"", &[(STREAMINFO, streaminfo(RATE)), (VORBIS_COMMENT, comments(&[]))]),
    )?;

    assert_eq!(chain::read_block(&path)?, None);
    Ok(())
}

/// `ingest::sort_unclaimed` says why this is an error rather than no sheet.
#[test]
fn a_native_block_that_does_not_parse_fails_the_read() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let path =
        written(&tmp, &flac(b"", &[(STREAMINFO, streaminfo(RATE)), (CUESHEET, vec![0; 100])]))?;

    assert!(chain::read_block(&path).is_err());
    Ok(())
}

/// A sheet comment placing two tracks, after `lead` lines of its own.
fn sheet_comment(lead: &str) -> String {
    format!(
        "{lead}FILE \"ripped.wav\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 04:00:00"
    )
}

fn vorbis(items: &[(&str, &str)]) -> VorbisComments {
    let mut comments = VorbisComments::default();
    for (key, value) in items {
        comments.push((*key).to_owned(), (*value).to_owned());
    }
    comments
}

#[test]
fn a_sheet_comment_is_read_without_its_bom() {
    let text = sheet_comment("REM GENRE \"Jazz\"\n");
    let comments = vorbis(&[("CUESHEET", &format!("\u{FEFF}{text}"))]);

    let read = read(Path::new("unread.flac"), Some(&comments));

    let expected = EmbeddedSheet { source: EmbeddedSource::Comment(text), overrides: Vec::new() };
    assert_eq!(read, EmbeddedCue::Present(expected));
}

#[test]
fn only_track_overrides_ride_with_the_sheet() {
    let text = sheet_comment("");
    let comments = vorbis(&[("CUESHEET", &text), ("TITLE", "Disc"), ("cue_track02_title", "Two")]);

    let read = read(Path::new("unread.flac"), Some(&comments));

    let overrides = vec![("cue_track02_title".to_owned(), "Two".to_owned())];
    let expected = EmbeddedSheet { source: EmbeddedSource::Comment(text), overrides };
    assert_eq!(read, EmbeddedCue::Present(expected));
}

#[test]
fn a_comment_placing_no_track_falls_to_the_native_block() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let block = two_track_block();
    let path =
        written(&tmp, &flac(b"", &[(STREAMINFO, streaminfo(RATE)), (CUESHEET, block.clone())]))?;
    let comments = vorbis(&[("CUESHEET", "REM GENRE \"Jazz\"")]);

    let read = read(&path, Some(&comments));

    let expected = EmbeddedSheet {
        source: EmbeddedSource::Block { sample_rate: RATE, bytes: block },
        overrides: Vec::new(),
    };
    assert_eq!(read, EmbeddedCue::Present(expected));
    Ok(())
}

#[test]
fn a_file_with_neither_reads_absent_and_one_that_will_not_read_unread() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let plain = written(&tmp, &flac(b"", &[(STREAMINFO, streaminfo(RATE))]))?;

    let found = (read(&plain, None), read(&tmp.path().join("gone.flac"), None));

    assert_eq!(found, (EmbeddedCue::Absent, EmbeddedCue::Unread));
    Ok(())
}

fn carved_titles(source: EmbeddedSource, overrides: &[(&str, &str)]) -> Option<Vec<String>> {
    let mut disc = make_test_metadata("Whole Disc");
    disc.duration_ms = 600_000;
    let overrides = overrides.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    disc.embedded_cue = EmbeddedCue::Present(EmbeddedSheet { source, overrides });
    let rows = carve(Path::new("album.flac"), &disc)?;
    Some(rows.into_iter().map(|row| row.meta.title).collect())
}

#[test]
fn a_sheet_comment_carves_its_file_whatever_its_file_lines_name() {
    let text =
        format!("{}\nFILE \"CD2.wav\" WAVE\nTRACK 03 AUDIO\nINDEX 01 06:00:00", sheet_comment(""));

    let titles = carved_titles(EmbeddedSource::Comment(text), &[("CUE_TRACK02_TITLE", "Two")]);

    let expected = ["album 01", "Two", "album 03"].map(str::to_owned).to_vec();
    assert_eq!(titles, Some(expected));
}

#[test]
fn a_native_block_carves_its_file_titled_by_its_overrides() {
    let block = EmbeddedSource::Block { sample_rate: RATE, bytes: two_track_block() };
    let titles = carved_titles(block, &[("CUE_TRACK01_TITLE", "One")]);
    assert_eq!(titles, Some(vec!["One".to_owned(), "album 02".to_owned()]));
}

#[test]
fn a_sheet_placing_one_track_carves_nothing() {
    let one = native_block("", &[audio_track(1, 0), lead_out(26_460_000)]);
    assert_eq!(carved_titles(EmbeddedSource::Block { sample_rate: RATE, bytes: one }, &[]), None);
}
