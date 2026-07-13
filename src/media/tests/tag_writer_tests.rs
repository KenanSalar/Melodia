//! Tag-writer tests.
//!
//! The pure-`apply_edit` cases need nothing on disk. The round-trips copy a real
//! fixture out of `tests/assets/` into a `TempDir` first — **never** write to the
//! checked-in asset, and note the assets deliberately do NOT live in
//! `tests/fixtures/`: `tests/headless.rs` adds that directory as a library folder
//! and asserts `scanned == 1`, and the scan is recursive, so any audio file added
//! under it breaks that test.

use std::path::{Path, PathBuf};

use lofty::file::TaggedFileExt;
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::{Tag, TagType};
use tempfile::TempDir;

use super::*;
use crate::error::AppError;
use crate::media::artwork;
use crate::media::metadata::extract_metadata;

// ---------------------------------------------------------------- helpers

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets")
}

/// Copy a checked-in fixture into `tmp` and hand back the working copy.
fn stage(tmp: &TempDir, name: &str) -> Result<PathBuf, AppError> {
    let src = assets_dir().join(name);
    let dst = tmp.path().join(name);
    std::fs::copy(&src, &dst)?;
    Ok(dst)
}

fn missing(what: &str) -> AppError {
    AppError::Validation(format!("missing {what}"))
}

/// Re-read a file's **primary** tag — the same tag the writer targets.
fn read_primary(path: &Path) -> Result<Tag, AppError> {
    let tagged = lofty::probe::read_from_path(path)
        .map_err(|e| AppError::metadata(format!("read {}", path.display()), e))?;
    let tag = tagged
        .primary_tag()
        .ok_or_else(|| missing("primary tag"))?
        .clone();
    Ok(tag)
}

fn text(tag: &Tag, key: ItemKey) -> Option<String> {
    tag.get_string(key).map(str::to_owned)
}

/// An edit that touches every field, so one call proves a whole container.
fn full_edit() -> TagEdit {
    TagEdit {
        title: FieldEdit::Set("New Title".into()),
        artist: FieldEdit::Set("New Artist".into()),
        album_artist: FieldEdit::Set("New Album Artist".into()),
        album: FieldEdit::Set("New Album".into()),
        genre: FieldEdit::Set("Shoegaze".into()),
        year: FieldEdit::Set(2024),
        original_year: FieldEdit::Set(1999),
        track_number: FieldEdit::Set(7),
        disc_number: FieldEdit::Set(2),
        composer: FieldEdit::Set("New Composer".into()),
        comment: FieldEdit::Set("New Comment".into()),
        bpm: FieldEdit::Set(128.0),
        lyrics: FieldEdit::Set("la la la".into()),
        artwork: ArtworkEdit::Keep,
    }
}

fn assert_full_edit_landed(tag: &Tag) -> Result<(), AppError> {
    assert_eq!(text(tag, ItemKey::TrackTitle).as_deref(), Some("New Title"));
    assert_eq!(
        text(tag, ItemKey::TrackArtist).as_deref(),
        Some("New Artist")
    );
    assert_eq!(
        text(tag, ItemKey::AlbumArtist).as_deref(),
        Some("New Album Artist")
    );
    assert_eq!(text(tag, ItemKey::AlbumTitle).as_deref(), Some("New Album"));
    assert_eq!(text(tag, ItemKey::Genre).as_deref(), Some("Shoegaze"));
    assert_eq!(
        text(tag, ItemKey::Composer).as_deref(),
        Some("New Composer")
    );
    assert_eq!(text(tag, ItemKey::Comment).as_deref(), Some("New Comment"));
    assert_eq!(tag.track(), Some(7));
    assert_eq!(tag.disk(), Some(2));

    let date = tag.date().ok_or_else(|| missing("date"))?;
    assert_eq!(date.year, 2024);

    let original = text(tag, ItemKey::OriginalReleaseDate).ok_or_else(|| missing("orig year"))?;
    assert!(
        original.starts_with("1999"),
        "original year should round-trip, got {original:?}"
    );
    Ok(())
}

// ------------------------------------------------------- pure `apply_edit`

#[test]
fn default_edit_is_a_noop() {
    assert!(TagEdit::default().is_noop());
    assert!(!TagEdit {
        title: FieldEdit::Set("x".into()),
        ..TagEdit::default()
    }
    .is_noop());
    // Artwork alone is enough to make it real work.
    assert!(!TagEdit {
        artwork: ArtworkEdit::Remove,
        ..TagEdit::default()
    }
    .is_noop());
}

#[test]
fn keep_leaves_the_existing_value_alone() {
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.insert_text(ItemKey::TrackTitle, "Original".into());

    let unsupported = apply_edit(&mut tag, &TagEdit::default(), None);

    assert!(unsupported.is_empty());
    assert_eq!(text(&tag, ItemKey::TrackTitle).as_deref(), Some("Original"));
}

#[test]
fn clear_removes_the_key_rather_than_writing_an_empty_string() {
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.insert_text(ItemKey::TrackTitle, "Original".into());

    let edit = TagEdit {
        title: FieldEdit::Clear,
        ..TagEdit::default()
    };
    apply_edit(&mut tag, &edit, None);

    // Not `Some("")` — a ghost empty tag is exactly what we must not produce.
    assert_eq!(text(&tag, ItemKey::TrackTitle), None);
}

#[test]
fn a_year_only_edit_preserves_an_existing_month_and_day() -> Result<(), AppError> {
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.set_date(Timestamp {
        year: 2001,
        month: Some(6),
        day: Some(15),
        ..Timestamp::default()
    });

    let edit = TagEdit {
        year: FieldEdit::Set(2024),
        ..TagEdit::default()
    };
    apply_edit(&mut tag, &edit, None);

    let date = tag.date().ok_or_else(|| missing("date"))?;
    assert_eq!(date.year, 2024);
    assert_eq!(date.month, Some(6), "month must survive a year-only edit");
    assert_eq!(date.day, Some(15), "day must survive a year-only edit");
    Ok(())
}

#[test]
fn keep_leaves_replaygain_and_musicbrainz_keys_untouched() {
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.insert_text(ItemKey::ReplayGainTrackGain, "-6.50 dB".into());
    tag.insert_text(ItemKey::MusicBrainzRecordingId, "mbid-123".into());

    // A real edit to an unrelated field must not disturb them.
    let edit = TagEdit {
        title: FieldEdit::Set("New".into()),
        ..TagEdit::default()
    };
    apply_edit(&mut tag, &edit, None);

    assert_eq!(
        text(&tag, ItemKey::ReplayGainTrackGain).as_deref(),
        Some("-6.50 dB")
    );
    assert_eq!(
        text(&tag, ItemKey::MusicBrainzRecordingId).as_deref(),
        Some("mbid-123")
    );
}

#[test]
fn bpm_writes_the_integer_key_on_id3v2_and_the_decimal_key_on_vorbis() {
    // ID3v2 has TBPM (IntegerBpm) but NO plain `Bpm` mapping.
    let mut id3 = Tag::new(TagType::Id3v2);
    let unsupported = apply_edit(
        &mut id3,
        &TagEdit {
            bpm: FieldEdit::Set(128.5),
            ..TagEdit::default()
        },
        None,
    );
    assert!(
        unsupported.is_empty(),
        "BPM must not be reported unsupported on ID3v2 — IntegerBpm maps"
    );
    assert_eq!(text(&id3, ItemKey::IntegerBpm).as_deref(), Some("129"));

    // Vorbis has BPM but NO IntegerBpm mapping.
    let mut vorbis = Tag::new(TagType::VorbisComments);
    let unsupported = apply_edit(
        &mut vorbis,
        &TagEdit {
            bpm: FieldEdit::Set(128.5),
            ..TagEdit::default()
        },
        None,
    );
    assert!(unsupported.is_empty());
    assert_eq!(text(&vorbis, ItemKey::Bpm).as_deref(), Some("128.5"));
}

#[test]
fn clearing_bpm_removes_both_keys() {
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.insert_text(ItemKey::Bpm, "128.5".into());
    tag.insert_text(ItemKey::IntegerBpm, "128".into());

    apply_edit(
        &mut tag,
        &TagEdit {
            bpm: FieldEdit::Clear,
            ..TagEdit::default()
        },
        None,
    );

    assert_eq!(text(&tag, ItemKey::Bpm), None);
    assert_eq!(text(&tag, ItemKey::IntegerBpm), None);
}

#[test]
fn lyrics_key_is_chosen_by_tag_type() {
    // Vorbis: must be LYRICS (what Picard/foobar2000/MusicBee read), not
    // UNSYNCEDLYRICS — where no other player would look.
    let mut vorbis = Tag::new(TagType::VorbisComments);
    apply_edit(
        &mut vorbis,
        &TagEdit {
            lyrics: FieldEdit::Set("words".into()),
            ..TagEdit::default()
        },
        None,
    );
    assert_eq!(text(&vorbis, ItemKey::Lyrics).as_deref(), Some("words"));

    // ID3v2 has no `Lyrics` mapping at all — it must land in USLT.
    let mut id3 = Tag::new(TagType::Id3v2);
    let unsupported = apply_edit(
        &mut id3,
        &TagEdit {
            lyrics: FieldEdit::Set("words".into()),
            ..TagEdit::default()
        },
        None,
    );
    assert!(unsupported.is_empty());
    assert_eq!(text(&id3, ItemKey::UnsyncLyrics).as_deref(), Some("words"));
}

#[test]
fn clearing_lyrics_removes_both_keys() {
    let mut tag = Tag::new(TagType::VorbisComments);
    tag.insert_text(ItemKey::Lyrics, "a".into());
    tag.insert_text(ItemKey::UnsyncLyrics, "b".into());

    apply_edit(
        &mut tag,
        &TagEdit {
            lyrics: FieldEdit::Clear,
            ..TagEdit::default()
        },
        None,
    );

    assert_eq!(text(&tag, ItemKey::Lyrics), None);
    assert_eq!(text(&tag, ItemKey::UnsyncLyrics), None);
}

// ------------------------------------------------- M4A: the `pic_type` trap

/// **The trap this whole module is shaped around.**
///
/// MP4 flattens every picture's `pic_type` to `Other` on read. A naive
/// `remove_picture_type(CoverFront)` therefore matches nothing, the tag ends up
/// holding `[old(Other), new(CoverFront)]`, both get written to `covr`, and on the
/// next read our own reader's "`CoverFront` > `CoverBack` > first available" fallback
/// picks up **the old cover**. Replace silently reverts.
///
/// This test fails against a `clear_front_cover` that only removes `CoverFront`.
#[test]
fn m4a_artwork_replace_actually_replaces() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence-cover.m4a")?;
    let artwork_dir = tmp.path().join("artwork");
    std::fs::create_dir(&artwork_dir)?;

    // Sanity: the fixture really does start with an embedded cover, else the trap
    // couldn't manifest and this test would pass vacuously.
    let before = read_primary(&audio)?;
    assert_eq!(
        before.pictures().len(),
        1,
        "fixture must carry exactly one embedded cover"
    );
    let old_bytes = before.pictures()[0].data().to_vec();

    let new_cover = assets_dir().join("cover.jpg");
    let picture = cover_picture_from_path(&new_cover)?;
    let new_bytes = picture.data().to_vec();
    assert_ne!(old_bytes, new_bytes, "fixtures must differ, or nothing is proven");

    let edit = TagEdit {
        artwork: ArtworkEdit::Replace,
        ..TagEdit::default()
    };
    let unsupported = apply_to_file(&audio, &edit, Some(&picture))?;
    assert!(unsupported.is_empty());

    // Exactly one picture — not the old one lingering beside the new one.
    let after = read_primary(&audio)?;
    assert_eq!(
        after.pictures().len(),
        1,
        "the old `Other`-typed cover must be gone, not merely joined by the new one"
    );
    assert_eq!(
        after.pictures()[0].data(),
        new_bytes.as_slice(),
        "the embedded cover must be the NEW image"
    );

    // And prove it end-to-end through the real reader, which is where the stale
    // cover would actually have surfaced.
    let cache = artwork::new_cover_cache();
    let meta = extract_metadata(&audio, &artwork_dir, &cache, false)?;
    let cached = meta.artwork_path.ok_or_else(|| missing("artwork_path"))?;
    let cached_bytes = std::fs::read(&cached)?;
    assert_eq!(
        cached_bytes, new_bytes,
        "find_and_cache_artwork must resolve to the new cover, not the stale one"
    );
    Ok(())
}

#[test]
fn m4a_artwork_remove_actually_removes() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence-cover.m4a")?;

    let edit = TagEdit {
        artwork: ArtworkEdit::Remove,
        ..TagEdit::default()
    };
    apply_to_file(&audio, &edit, None)?;

    let after = read_primary(&audio)?;
    assert!(
        after.pictures().is_empty(),
        "Remove must clear the `Other`-typed cover MP4 gives us"
    );
    Ok(())
}

/// A WebP cover must normalize to JPEG and **save**.
///
/// lofty's `Picture::from_reader` sniffs only PNG/JPEG/GIF/BMP/TIFF, so a WebP is
/// rejected outright — and even a format lofty *does* sniff (TIFF) would still be
/// refused by MP4's `covr` writer, which accepts only Gif/Jpeg/Png/Bmp. The
/// normalizer is what makes a WebP pick survivable at all, and M4A is the target
/// that proves it.
#[test]
fn m4a_accepts_a_webp_cover_by_normalizing_it_to_jpeg() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence.m4a")?;

    let webp = assets_dir().join("cover.webp");
    // Raw lofty refuses the WebP outright — this is the thing being worked around.
    let raw = std::fs::File::open(&webp)?;
    let mut reader = std::io::BufReader::new(raw);
    assert!(
        lofty::picture::Picture::from_reader(&mut reader).is_err(),
        "precondition: lofty must reject a raw WebP, else this test proves nothing"
    );

    let picture = cover_picture_from_path(&webp)?;
    assert_eq!(
        picture.mime_type(),
        Some(&lofty::picture::MimeType::Jpeg),
        "a WebP must be re-encoded to JPEG"
    );

    let edit = TagEdit {
        artwork: ArtworkEdit::Replace,
        ..TagEdit::default()
    };
    // The assertion is simply that this does not error: an un-normalized WebP
    // would trip MP4's `FileEncodingError` here.
    apply_to_file(&audio, &edit, Some(&picture))?;

    let after = read_primary(&audio)?;
    assert_eq!(after.pictures().len(), 1);
    Ok(())
}

#[test]
fn a_jpeg_cover_is_embedded_byte_for_byte() -> Result<(), AppError> {
    let jpg = assets_dir().join("cover.jpg");
    let original = std::fs::read(&jpg)?;

    let picture = cover_picture_from_path(&jpg)?;

    assert_eq!(
        picture.mime_type(),
        Some(&lofty::picture::MimeType::Jpeg),
        "JPEG must pass through, not be re-compressed"
    );
    assert_eq!(picture.data(), original.as_slice());
    assert_eq!(picture.pic_type(), PictureType::CoverFront);
    Ok(())
}

#[test]
fn a_corrupt_cover_is_rejected_before_anything_is_written() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let broken = tmp.path().join("truncated.jpg");
    // A valid JPEG magic number and nothing behind it. `Picture::from_reader`
    // sniffs 8 bytes and would happily embed this; only a real decode catches it.
    std::fs::write(&broken, [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46])?;

    assert!(
        cover_picture_from_path(&broken).is_err(),
        "the decode is the validation step — a truncated image must not embed"
    );
    Ok(())
}

// -------------------------------------------------------- FLAC (Vorbis)

#[test]
fn flac_round_trips_a_full_edit_with_lyrics_under_the_lyrics_key() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence.flac")?;

    let unsupported = apply_to_file(&audio, &full_edit(), None)?;
    assert!(
        unsupported.is_empty(),
        "VorbisComments maps every field: {:?}",
        unsupported.0
    );

    let tag = read_primary(&audio)?;
    assert_eq!(tag.tag_type(), TagType::VorbisComments);
    assert_full_edit_landed(&tag)?;

    // The interop-critical part: LYRICS, not UNSYNCEDLYRICS.
    assert_eq!(text(&tag, ItemKey::Lyrics).as_deref(), Some("la la la"));
    // And BPM as the decimal key, which is the only one Vorbis maps.
    assert_eq!(text(&tag, ItemKey::Bpm).as_deref(), Some("128"));
    Ok(())
}

/// The `read_cover_art` trap: `apply_to_file` must parse WITH pictures, or
/// `save_to_path` rewrites the tag from a picture-less parse and silently deletes
/// every embedded image.
#[test]
fn an_embedded_picture_survives_an_edit_that_does_not_touch_artwork() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence-cover.flac")?;

    let before = read_primary(&audio)?;
    assert_eq!(before.pictures().len(), 1, "fixture must carry a cover");
    let cover_bytes = before.pictures()[0].data().to_vec();

    // ArtworkEdit::Keep — we are editing text only.
    let edit = TagEdit {
        title: FieldEdit::Set("Retitled".into()),
        ..TagEdit::default()
    };
    apply_to_file(&audio, &edit, None)?;

    let after = read_primary(&audio)?;
    assert_eq!(
        after.pictures().len(),
        1,
        "the embedded picture must survive a text-only edit"
    );
    assert_eq!(after.pictures()[0].data(), cover_bytes.as_slice());
    Ok(())
}

// --------------------------------------------------------- MP3 (ID3v2)

#[test]
fn mp3_round_trips_a_full_edit_with_bpm_in_tbpm() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence.mp3")?;

    let unsupported = apply_to_file(&audio, &full_edit(), None)?;
    assert!(
        unsupported.is_empty(),
        "ID3v2 maps every field: {:?}",
        unsupported.0
    );

    let tag = read_primary(&audio)?;
    assert_eq!(tag.tag_type(), TagType::Id3v2);
    assert_full_edit_landed(&tag)?;

    // ID3v2 has no plain `Bpm` key — it must have gone to TBPM (IntegerBpm).
    assert_eq!(text(&tag, ItemKey::IntegerBpm).as_deref(), Some("128"));
    // And lyrics to USLT, since ID3v2 has no `Lyrics` mapping.
    assert_eq!(text(&tag, ItemKey::UnsyncLyrics).as_deref(), Some("la la la"));
    Ok(())
}

/// An MP3 carrying **only** an `ID3v1` tag must get a fresh `ID3v2` tag.
///
/// This is why the writer targets `primary_tag_type()` and never `first_tag_mut()`:
/// `ID3v1`'s whole key set is eight items, so applying the edit there would silently
/// drop album-artist, composer, BPM and lyrics — half the user's work, with no
/// error.
#[test]
fn an_id3v1_only_mp3_gains_a_fresh_id3v2_tag_and_loses_nothing() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence-id3v1.mp3")?;

    // Precondition: the fixture really has ID3v1 and no ID3v2, else this proves
    // nothing.
    let tagged = lofty::probe::read_from_path(&audio)
        .map_err(|e| AppError::metadata("read fixture", e))?;
    assert!(
        tagged.tag(TagType::Id3v2).is_none(),
        "fixture must not already carry an ID3v2 tag"
    );
    assert!(
        tagged.tag(TagType::Id3v1).is_some(),
        "fixture must carry an ID3v1 tag"
    );

    let unsupported = apply_to_file(&audio, &full_edit(), None)?;
    assert!(unsupported.is_empty(), "{:?}", unsupported.0);

    let tag = read_primary(&audio)?;
    assert_eq!(
        tag.tag_type(),
        TagType::Id3v2,
        "the edit must land in a fresh ID3v2 tag, not the ID3v1 one"
    );
    assert_full_edit_landed(&tag)?;

    // The four fields ID3v1 has no key for — the whole point of the fix.
    assert_eq!(
        text(&tag, ItemKey::AlbumArtist).as_deref(),
        Some("New Album Artist")
    );
    assert_eq!(
        text(&tag, ItemKey::Composer).as_deref(),
        Some("New Composer")
    );
    assert_eq!(text(&tag, ItemKey::IntegerBpm).as_deref(), Some("128"));
    assert_eq!(text(&tag, ItemKey::UnsyncLyrics).as_deref(), Some("la la la"));
    Ok(())
}

// ---------------------------------------------------------------- WAV

/// WAV's primary tag is **`ID3v2`**, not RIFF INFO.
///
/// It is tempting to read `RIFF_INFO_MAP` (no album-artist / disc / BPM / lyrics)
/// and expect a WAV edit to come back mostly-unsupported. It doesn't: `Id3v2Tag`
/// is writable in WAV, stored in an `ID3 ` chunk, and maps everything. The sparse
/// RIFF map is only reachable by explicitly targeting `TagType::RiffInfo` — which
/// this writer never does.
#[test]
fn wav_round_trips_a_full_edit_through_a_fresh_id3v2_tag() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let audio = stage(&tmp, "silence.wav")?;

    let unsupported = apply_to_file(&audio, &full_edit(), None)?;
    assert!(
        unsupported.is_empty(),
        "WAV's primary tag is ID3v2, which maps every field: {:?}",
        unsupported.0
    );

    let tag = read_primary(&audio)?;
    assert_eq!(tag.tag_type(), TagType::Id3v2);
    assert_full_edit_landed(&tag)?;
    Ok(())
}
