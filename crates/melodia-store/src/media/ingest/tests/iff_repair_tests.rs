//! WAV and AIFF chunk repair, over containers built byte by byte.
//!
//! A damaged file is made from a whole one the way the module's `//!` says the splice made it: the
//! byte it cost taken out, and the zero it left astray put back at the end. So each repair is held
//! to the exact file it started from rather than to a walk that merely reads clean.

use std::path::{Path, PathBuf};

use lofty::file::{FileType, TaggedFileExt};
use lofty::prelude::Accessor;
use tempfile::TempDir;

use super::*;
use crate::media::ingest::metadata::{TagScope, read_tags};
use crate::media::ingest::tag_writer::apply_to_file;
use melodia_core::entities::tags::{FieldEdit, TagEdit};
use melodia_core::error::AppError;

#[derive(Clone, Copy)]
enum Kind {
    Wav,
    Aiff,
}

impl Kind {
    fn size(self, len: usize) -> [u8; 4] {
        let len = u32::try_from(len).unwrap_or(u32::MAX);
        match self {
            Self::Wav => len.to_le_bytes(),
            Self::Aiff => len.to_be_bytes(),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aiff",
        }
    }

    /// The format chunk and the audio chunk, the audio holding `samples` as 8-bit mono frames.
    fn audio(self, samples: &[u8]) -> Vec<u8> {
        match self {
            Self::Wav => {
                // PCM, mono, 8 kHz, 8 kB/s, one byte a frame, 8 bits.
                let format = [1, 0, 1, 0, 0x40, 0x1F, 0, 0, 0x40, 0x1F, 0, 0, 1, 0, 8, 0];
                [chunk(self, *b"fmt ", &format), chunk(self, *b"data", samples)].concat()
            }
            Self::Aiff => {
                let frames = u8::try_from(samples.len()).unwrap_or(u8::MAX);
                // Mono, `frames` frames, 8 bits, 8 kHz as an 80-bit extended float.
                let common = [0, 1, 0, 0, 0, frames, 0, 8, 0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0];
                let sound = [&[0; 8][..], samples].concat();
                [chunk(self, *b"COMM", &common), chunk(self, *b"SSND", &sound)].concat()
            }
        }
    }
}

/// Odd, so the audio chunk ends on a pad byte, the slot a splice can land in.
const ODD_SAMPLES: &[u8] = &[0x10, 0x20, 0x30];
const EVEN_SAMPLES: &[u8] = &[0x10, 0x20, 0x30, 0x40];

/// Where the first tag chunk sits behind [`ODD_SAMPLES`]: the 12-byte header, then `fmt ` (24)
/// and `data` (12) in a WAV, `COMM` (26) and `SSND` (20) in an AIFF.
const WAV_TAG_AT: usize = 48;
const AIFF_TAG_AT: usize = 58;

/// `id`'s chunk holding `data`, followed by its pad byte where `data` is odd.
fn chunk(kind: Kind, id: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = [&id[..], &kind.size(data.len()), data].concat();
    if !data.len().is_multiple_of(2) {
        bytes.push(0);
    }
    bytes
}

/// A container of `chunks`, its size stated whole.
fn container(kind: Kind, chunks: &[u8]) -> Vec<u8> {
    let (id, form) = match kind {
        Kind::Wav => (b"RIFF", b"WAVE"),
        Kind::Aiff => (b"FORM", b"AIFF"),
    };
    [&id[..], &kind.size(chunks.len() + 4), form, chunks].concat()
}

/// A whole container of `samples` and then `tags`, each in an `ID3 ` chunk.
fn whole(kind: Kind, samples: &[u8], tags: &[Vec<u8>]) -> Vec<u8> {
    let mut chunks = kind.audio(samples);
    for tag in tags {
        chunks.extend(chunk(kind, *b"ID3 ", tag));
    }
    container(kind, &chunks)
}

fn stating(kind: Kind, mut bytes: Vec<u8>, stated: usize) -> Vec<u8> {
    bytes.splice(4..8, kind.size(stated));
    bytes
}

/// The size field four bytes into `bytes`, a container's or a chunk's, in `kind`'s order.
fn size_field(kind: Kind, bytes: &[u8]) -> Option<u32> {
    let size = *bytes.get(4..8)?.first_chunk::<4>()?;
    Some(match kind {
        Kind::Wav => u32::from_le_bytes(size),
        Kind::Aiff => u32::from_be_bytes(size),
    })
}

/// `bytes` as a header leaving out the final pad states it: an odd size one short of the file.
fn leaving_out_the_final_pad(kind: Kind, bytes: Vec<u8>) -> Vec<u8> {
    let stated = bytes.len() - 9;
    stating(kind, bytes, stated)
}

/// An `ID3v2.4` tag of UTF-8 text frames, 10 bytes plus 11 and the text's length a frame, so the
/// texts below pick its parity. Each is short enough that its synchsafe size is the plain one.
fn id3v2(frames: &[(&[u8; 4], &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, text) in frames {
        body.extend(*id);
        body.extend(synchsafe(text.len() + 1));
        body.extend([0, 0, 3]);
        body.extend(text.as_bytes());
    }
    [&b"ID3\x04\x00\x00"[..], &synchsafe(body.len()), &body].concat()
}

fn synchsafe(len: usize) -> [u8; 4] {
    [0, 0, 0, u8::try_from(len).unwrap_or(u8::MAX)]
}

/// 26 bytes, even.
fn even_tag() -> Vec<u8> {
    id3v2(&[(b"TIT2", "First")])
}

/// 27 bytes, odd.
fn odd_tag() -> Vec<u8> {
    id3v2(&[(b"TIT2", "Fourth")])
}

/// `whole` as the splice left it where its drain cut the `I` at `tag_at`.
fn cut_id(whole: &[u8], tag_at: usize) -> Vec<u8> {
    let mut damaged = whole.to_vec();
    damaged.remove(tag_at);
    damaged.push(0);
    damaged
}

/// `whole` as the splice left it where the chunk at `tag_at` landed in the pad slot before it.
fn landed_early(whole: &[u8], tag_at: usize) -> Vec<u8> {
    let mut damaged = whole.to_vec();
    damaged.remove(tag_at - 1);
    damaged.push(0);
    damaged
}

fn written(tmp: &TempDir, name: &str, bytes: &[u8]) -> Result<PathBuf, AppError> {
    let path = tmp.path().join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// The ids of the chunks `bytes` walks through, each stepped over with its pad.
fn chunk_ids(kind: Kind, bytes: &[u8]) -> Vec<[u8; 4]> {
    let mut ids = Vec::new();
    let mut at = 12;
    while let Some(header) = bytes.get(at..at + 8) {
        let (Some(&id), Some(size)) = (header.first_chunk::<4>(), size_field(kind, header)) else {
            break;
        };
        ids.push(id);
        let padded = u64::from(size) + u64::from(!size.is_multiple_of(2));
        at += 8 + usize::try_from(padded).unwrap_or(bytes.len());
    }
    ids
}

fn titled(title: &str) -> TagEdit {
    TagEdit { title: FieldEdit::Set(title.to_owned()), ..TagEdit::default() }
}

fn damage_in(path: &Path) -> Result<Damage, AppError> {
    match diagnose(path)? {
        Verdict::Damaged(damage) => Ok(damage),
        verdict => Err(AppError::metadata_msg(format!("expected damage, found {verdict:?}"))),
    }
}

// ------------------------------------------------------------ the prevention

/// Counting a pad anywhere else would state a byte the file doesn't hold or move a size that was
/// already right.
#[test]
fn only_a_size_leaving_out_the_final_pad_is_restated() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let mut wav_without_its_pad = whole(Kind::Wav, ODD_SAMPLES, &[]);
    wav_without_its_pad.pop();
    let rows: [(&str, Kind, FileType, Vec<u8>, u32); 5] = [
        (
            "a WAV leaving out its pad",
            Kind::Wav,
            FileType::Wav,
            leaving_out_the_final_pad(Kind::Wav, whole(Kind::Wav, ODD_SAMPLES, &[])),
            40,
        ),
        (
            "an AIFF leaving out its pad",
            Kind::Aiff,
            FileType::Aiff,
            leaving_out_the_final_pad(Kind::Aiff, whole(Kind::Aiff, ODD_SAMPLES, &[])),
            50,
        ),
        ("a WAV counting it", Kind::Wav, FileType::Wav, whole(Kind::Wav, ODD_SAMPLES, &[]), 40),
        (
            "a WAV whose last chunk lacks its pad byte",
            Kind::Wav,
            FileType::Wav,
            stating(Kind::Wav, wav_without_its_pad, 39),
            39,
        ),
        (
            "another format",
            Kind::Wav,
            FileType::Flac,
            leaving_out_the_final_pad(Kind::Wav, whole(Kind::Wav, ODD_SAMPLES, &[])),
            39,
        ),
    ];
    for (what, kind, file_type, bytes, expected) in rows {
        let path = written(&tmp, "track", &bytes)?;
        let mut copy = File::options().read(true).write(true).open(&path)?;

        include_final_pad(&mut copy, file_type)?;

        assert_eq!(size_field(kind, &std::fs::read(&path)?), Some(expected), "{what}");
    }
    Ok(())
}

/// Written twice, since a second write over a cut chunk grows a chain.
#[test]
fn a_tag_write_leaves_a_file_leaving_out_its_final_pad_walking_clean() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let rows = [
        ("a WAV ending on odd audio", Kind::Wav, whole(Kind::Wav, ODD_SAMPLES, &[])),
        ("a WAV ending on an odd tag", Kind::Wav, whole(Kind::Wav, EVEN_SAMPLES, &[odd_tag()])),
        ("an AIFF ending on odd audio", Kind::Aiff, whole(Kind::Aiff, ODD_SAMPLES, &[])),
        ("an AIFF ending on an odd tag", Kind::Aiff, whole(Kind::Aiff, EVEN_SAMPLES, &[odd_tag()])),
    ];
    for (what, kind, bytes) in rows {
        let bytes = leaving_out_the_final_pad(kind, bytes);
        let path = written(&tmp, &format!("track.{}", kind.extension()), &bytes)?;

        apply_to_file(&path, &titled("Once"), None)?;
        apply_to_file(&path, &titled("Twice"), None)?;

        assert_eq!(diagnose(&path)?, Verdict::Clean, "{what}");
    }
    Ok(())
}

// ------------------------------------------------------------ the diagnosis

#[test]
fn a_file_whose_chunks_walk_to_its_end_is_clean() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let rows = [
        ("a WAV", whole(Kind::Wav, ODD_SAMPLES, &[])),
        ("an AIFF", whole(Kind::Aiff, ODD_SAMPLES, &[])),
        ("a WAV ending on a tag", whole(Kind::Wav, ODD_SAMPLES, &[odd_tag()])),
        ("an AIFF ending on a tag", whole(Kind::Aiff, ODD_SAMPLES, &[odd_tag()])),
        ("a file that is no container", id3v2(&[(b"TIT2", "An MP3's tag")])),
        ("a file too short for a header", b"RIFF\x04\0\0\0WAV".to_vec()),
    ];
    for (what, bytes) in rows {
        let path = written(&tmp, "track", &bytes)?;

        assert_eq!(diagnose(&path)?, Verdict::Clean, "{what}");
    }
    Ok(())
}

/// The repair touches only what the splice leaves. Each of these breaks the walk some other way,
/// and mending it would be guessing at a file no tag write explains.
#[test]
fn a_walk_breaking_the_way_no_splice_does_is_unrecognised() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let tagged = whole(Kind::Wav, ODD_SAMPLES, &[even_tag()]);
    let cut = cut_id(&tagged, WAV_TAG_AT);
    let data_at = WAV_TAG_AT - 12;

    let mut overlong = tagged.clone();
    overlong.splice(data_at + 4..data_at + 8, Kind::Wav.size(100));
    // The cut chunk's tag header sits 7 bytes in, its size in the last 4 of its 10.
    let mut tag_disagreeing = cut.clone();
    tag_disagreeing[WAV_TAG_AT + 7 + 9] += 1;
    let mut two_strays = cut.clone();
    two_strays.push(0);
    let list = chunk(Kind::Wav, *b"LIST", b"INFO");
    let mut before_another_chunk = cut.clone();
    before_another_chunk.splice(cut.len() - 1..cut.len() - 1, list);

    let rows = [
        ("a chunk running past the end", overlong, data_at),
        ("a cut chunk whose tag states another length", tag_disagreeing, WAV_TAG_AT),
        ("a cut chunk with two stray zeros after it", two_strays, WAV_TAG_AT),
        ("a cut chunk ahead of a chunk no tag write leaves", before_another_chunk, WAV_TAG_AT),
    ];
    for (what, bytes, at) in rows {
        let path = written(&tmp, "track.wav", &bytes)?;

        let at = u64::try_from(at).unwrap_or(u64::MAX);
        assert_eq!(diagnose(&path)?, Verdict::Unrecognised { at }, "{what}");
    }
    Ok(())
}

// --------------------------------------------------------------- the repair

/// Both shapes in both containers, the cut after an odd chunk and after an even one, each damaged
/// file stating the odd size its header was written with.
#[test]
fn a_repair_gives_back_the_file_before_the_splice() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let wav = whole(Kind::Wav, ODD_SAMPLES, &[even_tag()]);
    let wav_even = whole(Kind::Wav, EVEN_SAMPLES, &[even_tag()]);
    let aiff = whole(Kind::Aiff, ODD_SAMPLES, &[even_tag()]);
    let rows = [
        ("a WAV's cut id", Kind::Wav, cut_id(&wav, WAV_TAG_AT), wav.clone()),
        ("a WAV's cut id after even audio", Kind::Wav, cut_id(&wav_even, WAV_TAG_AT), wav_even),
        ("a WAV's chunk landed early", Kind::Wav, landed_early(&wav, WAV_TAG_AT), wav),
        ("an AIFF's cut id", Kind::Aiff, cut_id(&aiff, AIFF_TAG_AT), aiff.clone()),
        ("an AIFF's chunk landed early", Kind::Aiff, landed_early(&aiff, AIFF_TAG_AT), aiff),
    ];
    for (what, kind, damaged, whole) in rows {
        let damaged = leaving_out_the_final_pad(kind, damaged);
        let path = written(&tmp, &format!("track.{}", kind.extension()), &damaged)?;

        repair(&path, damage_in(&path)?)?;

        assert_eq!(std::fs::read(&path)?, whole, "{what}");
    }
    Ok(())
}

/// A cut-id file written again, the next chunk spliced in ahead of the stray byte or past it.
fn chains() -> [(&'static str, Vec<u8>); 2] {
    let first = id3v2(&[(b"TIT2", "First"), (b"TALB", "Album")]);
    let second = id3v2(&[(b"TIT2", "Second")]);
    let chained = whole(Kind::Wav, ODD_SAMPLES, &[first.clone(), second]);
    let second_at = WAV_TAG_AT + chunk(Kind::Wav, *b"ID3 ", &first).len();

    let mut past_the_stray = chained.clone();
    past_the_stray.remove(WAV_TAG_AT);
    past_the_stray.insert(second_at - 1, 0);
    [
        ("a chunk spliced ahead of the stray byte", cut_id(&chained, WAV_TAG_AT)),
        ("a chunk appended past it", past_the_stray),
    ]
}

/// `TagLib` reads the first ID3 chunk alone, which holds none of the edits made after the damage.
#[test]
fn a_repaired_chain_is_one_tag_chunk() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    for (what, damaged) in chains() {
        let path = written(&tmp, "track.wav", &damaged)?;

        repair(&path, damage_in(&path)?)?;

        assert_eq!(
            chunk_ids(Kind::Wav, &std::fs::read(&path)?),
            [*b"fmt ", *b"data", *b"ID3 "],
            "{what}"
        );
    }
    Ok(())
}

/// lofty's merge of a chain lets a later chunk's frame win.
#[test]
fn a_folded_chain_keeps_the_later_edit_and_the_first_chunks_own_fields() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    for (what, damaged) in chains() {
        let path = written(&tmp, "track.wav", &damaged)?;

        repair(&path, damage_in(&path)?)?;

        let tagged = read_tags(&path, TagScope::TagsOnly)?;
        let tag = tagged.primary_tag().ok_or_else(|| AppError::metadata_msg("no tag"))?;
        let fields = (tag.title().map(String::from), tag.album().map(String::from));
        assert_eq!(fields, (Some("Second".to_owned()), Some("Album".to_owned())), "{what}");
    }
    Ok(())
}

/// The damage is read once and mended on a copy made later. Here the file was damaged the other
/// way in between: mended at the old offsets it would walk clean and play the same, with an `I`
/// left in the audio chunk's pad slot.
#[test]
fn a_file_damaged_otherwise_since_its_diagnosis_is_left_as_it_was() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let wav = whole(Kind::Wav, ODD_SAMPLES, &[even_tag()]);
    let path = written(&tmp, "track.wav", &cut_id(&wav, WAV_TAG_AT))?;
    let damage = damage_in(&path)?;
    let since = landed_early(&wav, WAV_TAG_AT);
    std::fs::write(&path, &since)?;

    let refused = repair(&path, damage);

    assert!(matches!(refused, Err(AppError::Metadata { .. })), "{refused:?}");
    assert_eq!(std::fs::read(&path)?, since);
    Ok(())
}
