//! The check a tag write's copy is held to, over fixtures from `test-assets/` and copies of them
//! edited byte by byte. Each copy sits at a path with no extension, as the rewrite's temp does.

use std::fs::File;
use std::path::PathBuf;

use tempfile::TempDir;

use super::*;
use crate::player::source::tests::helpers::asset;
use melodia_core::error::AppError;
use melodia_testkit::restate_ogg_checksum;

fn copy_of(
    tmp: &TempDir,
    name: &str,
    edit: impl FnOnce(&mut Vec<u8>),
) -> Result<PathBuf, AppError> {
    let mut bytes = std::fs::read(asset(name))?;
    edit(&mut bytes);
    let copy = tmp.path().join(name.replace('.', "-"));
    std::fs::write(&copy, bytes)?;
    Ok(copy)
}

fn written(tmp: &TempDir, name: &str, bytes: &[u8]) -> Result<PathBuf, AppError> {
    let path = tmp.path().join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Every format lofty writes into.
const WRITTEN_FORMATS: [&str; 9] = [
    "silence.wav",
    "silence.aiff",
    "silence.flac",
    "silence.mp3",
    "silence.ogg",
    "silence.opus",
    "silence.m4a",
    "silence.aac",
    "silence-id3v1.mp3",
];

/// A check refusing a copy it should pass refuses every tag edit on that format.
#[test]
fn a_byte_for_byte_copy_matches_its_original() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    for name in WRITTEN_FORMATS {
        let copy = copy_of(&tmp, name, |_| {})?;

        let verdict = compare(&asset(name), &copy);

        assert!(matches!(verdict, AudioMatch::Same), "{name}: {verdict:?}");
    }
    Ok(())
}

/// The shape a writer that stops copying at a page it can't parse leaves.
#[test]
fn a_copy_cut_short_is_refused() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    for name in WRITTEN_FORMATS {
        let copy = copy_of(&tmp, name, |bytes| bytes.truncate(bytes.len() * 9 / 10))?;

        let verdict = compare(&asset(name), &copy);

        assert!(matches!(verdict, AudioMatch::Changed(_)), "{name}: {verdict:?}");
    }
    Ok(())
}

#[test]
fn a_copy_whose_samples_changed_is_refused() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let copy = copy_of(&tmp, "silence.wav", |bytes| {
        let middle = bytes.len() / 2;
        bytes[middle] = 0x7F;
    })?;

    let verdict = compare(&asset("silence.wav"), &copy);

    assert!(matches!(verdict, AudioMatch::Changed(AudioChange::Packet(_))), "{verdict:?}");
    Ok(())
}

/// The last byte of the STREAMINFO MD5 sits 41 bytes in: `fLaC`, the block header, then 34 bytes
/// ending on the 16-byte signature.
#[test]
fn a_changed_stream_header_is_refused_though_every_packet_matches() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let copy = copy_of(&tmp, "silence.flac", |bytes| bytes[41] ^= 0xFF)?;

    let verdict = compare(&asset("silence.flac"), &copy);

    assert!(matches!(verdict, AudioMatch::Changed(AudioChange::Tracks)), "{verdict:?}");
    Ok(())
}

#[test]
fn a_copy_holding_another_stream_is_refused() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let other = std::fs::read(asset("silence-32bit.wav"))?;
    let copy = written(&tmp, "copy", &other)?;

    let verdict = compare(&asset("silence.wav"), &copy);

    assert!(matches!(verdict, AudioMatch::Changed(AudioChange::Tracks)), "{verdict:?}");
    Ok(())
}

#[test]
fn a_copy_that_wont_open_is_refused() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let copy = written(&tmp, "copy", b"not audio at all")?;

    let verdict = compare(&asset("silence.flac"), &copy);

    assert!(matches!(verdict, AudioMatch::Changed(AudioChange::Unreadable(_))), "{verdict:?}");
    Ok(())
}

#[test]
fn an_original_that_wont_open_leaves_its_copy_unchecked() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let original = written(&tmp, "track.mp3", b"not audio at all")?;
    let copy = written(&tmp, "copy", b"not audio at all")?;

    let verdict = compare(&original, &copy);

    assert!(matches!(verdict, AudioMatch::Unchecked(_)), "{verdict:?}");
    Ok(())
}

/// ADTS ends in an error where MPEG resyncs, and 454 bytes of the fixture stops two bytes into a
/// frame's header.
#[test]
fn a_copy_of_an_original_cut_short_matches_it() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let whole = std::fs::read(asset("silence.aac"))?;
    let cut = whole.get(..454).ok_or_else(|| AppError::player_msg("the fixture is too short"))?;
    let original = written(&tmp, "cut.aac", cut)?;
    let copy = written(&tmp, "copy", cut)?;

    let verdict = compare(&original, &copy);

    assert!(matches!(verdict, AudioMatch::Same), "{verdict:?}");
    Ok(())
}

/// Two links, each a whole stream with a serial of its own: `silence.ogg`'s and `silence.oga`'s.
fn chained_ogg() -> Result<Vec<u8>, AppError> {
    Ok([std::fs::read(asset("silence.ogg"))?, std::fs::read(asset("silence.oga"))?].concat())
}

#[test]
fn a_copy_of_a_chained_ogg_matches_it_across_its_links() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let original = written(&tmp, "chained.ogg", &chained_ogg()?)?;
    let copy = written(&tmp, "copy", &chained_ogg()?)?;

    let verdict = compare(&original, &copy);

    assert!(matches!(verdict, AudioMatch::Same), "{verdict:?}");
    Ok(())
}

/// Each link holds its audio in one page, the file's last for the second link. Changed there, with
/// the page's checksum restated so it still reads, the copy differs only past the first link.
#[test]
fn a_chained_ogg_copy_changed_in_its_second_link_is_refused() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let mut changed = chained_ogg()?;
    let last_page = changed
        .windows(4)
        .rposition(|bytes| bytes == b"OggS")
        .ok_or_else(|| AppError::player_msg("the chain holds no Ogg page"))?;
    let last_byte = changed.len() - 1;
    changed[last_byte] ^= 0xFF;
    restate_ogg_checksum(&mut changed[last_page..]);
    let original = written(&tmp, "chained.ogg", &chained_ogg()?)?;
    let copy = written(&tmp, "copy", &changed)?;

    let verdict = compare(&original, &copy);

    assert!(matches!(verdict, AudioMatch::Changed(AudioChange::Packet(_))), "{verdict:?}");
    Ok(())
}

// ------------------------------------------------------------- the bound

/// The `APEv2` flags for a tag carrying a header, and for the header itself.
const HAS_HEADER: u32 = 1 << 31;
const IS_HEADER: u32 = 1 << 29;

/// An `APEv2` footer over `items` bytes of items, or with [`IS_HEADER`] the header.
fn ape_block(items: usize, flags: u32) -> Vec<u8> {
    let size = u32::try_from(items + 32).unwrap_or(u32::MAX);
    let fields: [&[u8]; 6] = [
        b"APETAGEX",
        &2000_u32.to_le_bytes(),
        &size.to_le_bytes(),
        &1_u32.to_le_bytes(),
        &flags.to_le_bytes(),
        &[0; 8],
    ];
    fields.concat()
}

fn ape_tag(items: usize) -> Vec<u8> {
    let header = ape_block(items, HAS_HEADER | IS_HEADER);
    [header, vec![b'i'; items], ape_block(items, HAS_HEADER)].concat()
}

fn ape_tag_without_header(items: usize) -> Vec<u8> {
    [vec![b'i'; items], ape_block(items, 0)].concat()
}

fn id3v1_tag() -> Vec<u8> {
    [b"TAG".as_slice(), &[b't'; 125]].concat()
}

#[test]
fn the_audio_ends_ahead_of_the_trailing_tags() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = vec![0x55; 1_000];
    let rows = [
        ("no tag", audio.clone(), 1_000),
        ("an ID3v1 tag", [audio.clone(), id3v1_tag()].concat(), 1_000),
        ("an APEv2 tag with a header", [audio.clone(), ape_tag(10)].concat(), 1_000),
        ("an APEv2 tag without one", [audio.clone(), ape_tag_without_header(10)].concat(), 1_000),
        (
            "an APEv2 tag ahead of an ID3v1 tag",
            [audio.clone(), ape_tag(10), id3v1_tag()].concat(),
            1_000,
        ),
        ("an ID3v1 tag and nothing ahead of it", id3v1_tag(), 0),
        ("a file one byte short of an ID3v1 tag", id3v1_tag()[1..].to_vec(), 127),
    ];
    for (what, bytes, expected) in rows {
        let path = written(&tmp, "track", &bytes)?;
        let mut file = File::open(&path)?;

        assert_eq!(audio_end(&mut file)?, expected, "{what}");
    }
    Ok(())
}

/// The tag's items here hold `silence.mp3`'s own frame header, MPEG-1 layer III at 56 kb/s, 44.1
/// kHz, mono, which a demuxer reading past the audio takes for a frame the original lacks.
#[test]
fn a_copy_whose_trailing_tag_changed_still_matches() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = std::fs::read(asset("silence.mp3"))?;
    let mut framed_tag = ape_tag(600);
    framed_tag[40..44].copy_from_slice(&[0xFF, 0xFB, 0x40, 0xC0]);
    let original = written(&tmp, "track.mp3", &[audio.clone(), ape_tag(600)].concat())?;
    let copy = written(&tmp, "copy", &[audio, framed_tag].concat())?;

    let verdict = compare(&original, &copy);

    assert!(matches!(verdict, AudioMatch::Same), "{verdict:?}");
    Ok(())
}
