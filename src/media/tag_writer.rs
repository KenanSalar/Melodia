//! Writing tags back to audio files (lofty): a per-field tri-state edit (`Keep` / `Clear` / `Set`)
//! applied to a file's **primary** tag, plus the cover-art normalizer that turns a user-picked
//! image into something every container we ship can store.
//!
//! Nothing here touches the DB, the UI or `AppState`. [`apply_edit`] is pure, which is what makes
//! the per-format key mappings below testable; [`apply_to_file`] is the blocking
//! read-modify-write around it and every caller goes through `spawn_blocking`.
//!
//! ## Why the primary tag, and only the primary tag
//!
//! [`TaggedFileExt::primary_tag_type`] is the format's canonical tag — `ID3v2`,
//! `VorbisComments` or `Ilst`, the whole set across every container Melodia scans that lofty can
//! tag, and all three map every field this module exposes. **Never `first_tag_mut()`**, which
//! `.claude/rules/library-data.md` argues; creating a fresh primary tag instead also matches the
//! reader (`metadata.rs` reads `primary_tag().or(first_tag())`), so the next `extract_metadata`
//! reads back what we wrote. **Never [`Tag::re_map`]** either — it discards the format-specific
//! companion tag, throwing away every frame with no `ItemKey`: `ReplayGain`, `MusicBrainz` ids,
//! `POPM`.
//!
//! ## What survives an edit
//!
//! `GlobalOptions::preserve_format_specific_items` defaults on, stashing those keyless frames in a
//! companion tag and merging them back on save — so `FieldEdit::Keep` genuinely means keep for
//! fields this module never heard of.
//!
//! `TaggedFile::save_to` re-serializes *every* tag in the file, so an MP3's companion `ID3v1` or a
//! WAV's RIFF INFO chunk survives, rewritten unchanged and now stale against the primary tag. That
//! is the trade, and it is why `WriteOptions::default()`'s `remove_others: false` must stay:
//! flipping it strips those tags outright, a bigger change than a stale `ID3v1`.

use std::io::Cursor;
use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::items::Timestamp;
use lofty::tag::{ItemValue, Tag, TagItem, TagType};

use super::{image_decode, metadata};
use crate::error::AppError;

/// Upper bound for a written BPM. Anything past this is a typo, not a tempo, and a tag holding a
/// 12-digit "tempo" is worse than one holding none.
const MAX_BPM: f64 = 1000.0;

/// A per-field tri-state. The dialog reports what the user *did*, not just the value they left
/// behind, because empty is not clear: `extract_metadata` filters whitespace-only tags to `None`,
/// so writing `""` leaves a ghost tag our own reader ignores and other players happily display.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FieldEdit<T> {
    /// Never touched — leave the file's tag exactly as it is.
    #[default]
    Keep,
    /// Emptied — remove the tag key entirely.
    Clear,
    Set(T),
}

/// Artwork is its own tri-state: the "value" is a decoded image, not a string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ArtworkEdit {
    #[default]
    Keep,
    Remove,
    /// Embed the picture the caller built with [`cover_picture_from_path`].
    Replace,
}

/// One dialog's worth of edits. Every field defaults to [`FieldEdit::Keep`], so a caller only sets
/// what the user actually changed.
#[derive(Debug, Clone, Default)]
pub struct TagEdit {
    pub title: FieldEdit<String>,
    pub artist: FieldEdit<String>,
    pub album_artist: FieldEdit<String>,
    pub album: FieldEdit<String>,
    pub genre: FieldEdit<String>,
    /// lofty's `Timestamp.year` is `u16`, so the form's year is parsed to `u16`.
    pub year: FieldEdit<u16>,
    pub original_year: FieldEdit<u16>,
    pub track_number: FieldEdit<u32>,
    pub disc_number: FieldEdit<u32>,
    pub composer: FieldEdit<String>,
    pub comment: FieldEdit<String>,
    /// Written by the auto-tag backfill so `ListenBrainz` loves — which key on it — work. Not
    /// surfaced in the Edit-Tags dialog.
    pub musicbrainz_track_id: FieldEdit<String>,
    pub bpm: FieldEdit<f64>,
    pub lyrics: FieldEdit<String>,
    pub artwork: ArtworkEdit,
}

impl TagEdit {
    /// True when the user changed nothing at all; the caller short-circuits on it. lofty rewrites
    /// the tag whether or not anything differs, so a reflexive open-then-Save on a 200-track album
    /// would otherwise rewrite 200 files — and, through the watcher, risk re-ingesting them.
    pub fn is_noop(&self) -> bool {
        self.title == FieldEdit::Keep
            && self.artist == FieldEdit::Keep
            && self.album_artist == FieldEdit::Keep
            && self.album == FieldEdit::Keep
            && self.genre == FieldEdit::Keep
            && self.year == FieldEdit::Keep
            && self.original_year == FieldEdit::Keep
            && self.track_number == FieldEdit::Keep
            && self.disc_number == FieldEdit::Keep
            && self.composer == FieldEdit::Keep
            && self.comment == FieldEdit::Keep
            && self.musicbrainz_track_id == FieldEdit::Keep
            && self.bpm == FieldEdit::Keep
            && self.lyrics == FieldEdit::Keep
            && self.artwork == ArtworkEdit::Keep
    }
}

/// Fields the file's tag format has no key for. Never an error — the rest of the edit still lands
/// — but the user is told, so "BPM didn't save" is a message rather than a mystery.
///
/// A safety net rather than a routine outcome: all three primary tag types map every field
/// [`TagEdit`] exposes. Don't build UI around it being populated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnsupportedFields(pub Vec<&'static str>);

impl UnsupportedFields {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Write one text item, recording the field name when the key doesn't map.
///
/// **Every write goes through here**, because [`Tag::insert_text`] silently drops an item whose
/// `ItemKey` has no mapping for the target `TagType` and says so only in its return — the bool
/// that makes the BPM and lyrics fallbacks below expressible. It is also why the [`Accessor`]
/// setters are unused: they throw that bool away, and their default bodies are empty no-ops, so a
/// non-overriding impl would discard the whole write.
fn set_text(
    tag: &mut Tag,
    key: ItemKey,
    value: String,
    field: &'static str,
    out: &mut Vec<&'static str>,
) {
    if !tag.insert_text(key, value) {
        out.push(field);
    }
}

/// Apply a simple string field: `Set` inserts, `Clear` removes, `Keep` does nothing.
fn apply_string(
    tag: &mut Tag,
    edit: &FieldEdit<String>,
    key: ItemKey,
    field: &'static str,
    out: &mut Vec<&'static str>,
) {
    match edit {
        FieldEdit::Keep => {}
        FieldEdit::Clear => tag.remove_key(key),
        FieldEdit::Set(v) => set_text(tag, key, v.clone(), field, out),
    }
}

/// Write the `MusicBrainz` **Recording** id.
///
/// Its own function because `ID3v2` stores it in a binary `UFID` frame, which
/// [`Tag::insert_text`]'s support check has no mapping for and would refuse. `insert_unchecked`
/// stores it regardless — lofty's `Tag → Id3v2Tag` conversion makes the `UFID` frame on save — and
/// `VorbisComments` and MP4 map the key directly, so the same path writes them too.
fn apply_recording_id(tag: &mut Tag, edit: &FieldEdit<String>) {
    match edit {
        FieldEdit::Keep => {}
        FieldEdit::Clear => tag.remove_key(ItemKey::MusicBrainzRecordingId),
        FieldEdit::Set(v) => tag.insert_unchecked(TagItem::new(
            ItemKey::MusicBrainzRecordingId,
            ItemValue::Text(v.clone()),
        )),
    }
}

/// Apply a numeric field that crosses the tag boundary as its decimal string.
fn apply_number<T: std::fmt::Display>(
    tag: &mut Tag,
    edit: &FieldEdit<T>,
    key: ItemKey,
    field: &'static str,
    out: &mut Vec<&'static str>,
) {
    match edit {
        FieldEdit::Keep => {}
        FieldEdit::Clear => tag.remove_key(key),
        FieldEdit::Set(v) => set_text(tag, key, v.to_string(), field, out),
    }
}

/// Remove the front cover — **both** `CoverFront` and `Other`.
///
/// MP4 has no picture-type concept: `Ilst` flattens every `pic_type` to [`PictureType::Other`] on
/// read, so keying the removal on `CoverFront` alone matches nothing on an M4A, and our own
/// `CoverFront > CoverBack > first` reader then falls through to the *old* cover — a Replace that
/// silently reverts and a Remove that does nothing. lofty 0.24 has no clear-all, so two calls.
///
/// Accepted collateral: a FLAC deliberately storing a non-cover image as `Other` loses it.
/// `CoverBack`, booklet and artist pictures survive, and Melodia's data model has one cover per
/// track anyway.
fn clear_front_cover(tag: &mut Tag) {
    tag.remove_picture_type(PictureType::CoverFront);
    tag.remove_picture_type(PictureType::Other);
}

/// BPM: the key differs per format, and `ItemKey::Bpm` **does not exist on `ID3v2`** while
/// `IntegerBpm` has no Vorbis mapping — so `insert_text(Bpm, …)` on an MP3 is a no-op returning
/// `false`, and `insert_text(IntegerBpm, …)` on a FLAC is the same.
///
/// Write `IntegerBpm` always, `Bpm` additionally where it maps, and report BPM unsupported only
/// when *both* come back `false` — the one field where that bool is load-bearing on a format we
/// ship.
fn apply_bpm(tag: &mut Tag, edit: &FieldEdit<f64>, out: &mut Vec<&'static str>) {
    match edit {
        FieldEdit::Keep => {}
        FieldEdit::Clear => {
            tag.remove_key(ItemKey::Bpm);
            tag.remove_key(ItemKey::IntegerBpm);
        }
        FieldEdit::Set(v) => {
            // Bound once and write the *same* value to both keys — the integer and decimal forms
            // of one BPM must not disagree. `f64::clamp` does not absorb NaN (both its comparisons
            // are false) and `str::parse::<f64>()` accepts "nan"/"inf", hence the explicit guard;
            // and `.round()` before formatting is load-bearing, `{:.0}` rounding half-to-even
            // where half-away-from-zero is what "rounded BPM" means everywhere else.
            let bpm = if v.is_nan() {
                0.0
            } else {
                v.clamp(0.0, MAX_BPM)
            };
            let int_ok = tag.insert_text(ItemKey::IntegerBpm, format!("{:.0}", bpm.round()));
            let dec_ok = tag.insert_text(ItemKey::Bpm, bpm.to_string());
            if !int_ok && !dec_ok {
                out.push("bpm");
            }
        }
    }
}

/// Lyrics: `ItemKey::Lyrics` still exists — it's **`ID3v2`** that lacks the mapping, the key being
/// overloaded there across `SYLT`/`USLT`.
///
/// `LYRICS` is what Picard, `foobar2000` and `MusicBee` write in Vorbis comments, so writing only
/// `UnsyncLyrics` would put our FLAC lyrics under `UNSYNCEDLYRICS` where no other player looks —
/// and leave theirs invisible to us. Write keyed by tag type, clear *both*.
fn apply_lyrics(tag: &mut Tag, edit: &FieldEdit<String>, out: &mut Vec<&'static str>) {
    match edit {
        FieldEdit::Keep => {}
        FieldEdit::Clear => {
            tag.remove_key(ItemKey::Lyrics);
            tag.remove_key(ItemKey::UnsyncLyrics);
        }
        FieldEdit::Set(v) => {
            let key = if tag.tag_type() == TagType::VorbisComments {
                ItemKey::Lyrics
            } else {
                ItemKey::UnsyncLyrics
            };
            set_text(tag, key, v.clone(), "lyrics", out);
        }
    }
}

/// Year, done by hand: [`Accessor`] exposes `date: Timestamp` rather than `year`, and `set_date`
/// discards the `insert_text` bool — so do what it does with the bool visible.
///
/// Seeding from the existing `tag.date()` is what preserves a month/day through a year-only edit;
/// `Timestamp`'s `Display` appends `-MM-DD` only when those parts are present.
fn apply_year(tag: &mut Tag, edit: &FieldEdit<u16>, out: &mut Vec<&'static str>) {
    match edit {
        FieldEdit::Keep => {}
        // Removes both `Year` and `RecordingDate`.
        FieldEdit::Clear => tag.remove_date(),
        FieldEdit::Set(y) => {
            let existing = tag.date().unwrap_or_default();
            let ts = Timestamp {
                year: *y,
                ..existing
            };
            tag.remove_key(ItemKey::Year);
            set_text(tag, ItemKey::RecordingDate, ts.to_string(), "year", out);
        }
    }
}

/// Apply `edit` to an in-memory tag, returning the fields this tag format had no key for.
/// **Pure — no I/O at all**; the `Replace` image is decoded by the caller and handed in built.
pub fn apply_edit(tag: &mut Tag, edit: &TagEdit, picture: Option<&Picture>) -> UnsupportedFields {
    let mut out: Vec<&'static str> = Vec::new();

    apply_string(tag, &edit.title, ItemKey::TrackTitle, "title", &mut out);
    apply_string(tag, &edit.artist, ItemKey::TrackArtist, "artist", &mut out);
    apply_string(tag, &edit.album_artist, ItemKey::AlbumArtist, "album_artist", &mut out);
    apply_string(tag, &edit.album, ItemKey::AlbumTitle, "album", &mut out);
    apply_string(tag, &edit.genre, ItemKey::Genre, "genre", &mut out);
    apply_string(tag, &edit.composer, ItemKey::Composer, "composer", &mut out);
    apply_string(tag, &edit.comment, ItemKey::Comment, "comment", &mut out);
    apply_recording_id(tag, &edit.musicbrainz_track_id);

    apply_number(tag, &edit.track_number, ItemKey::TrackNumber, "track_number", &mut out);
    apply_number(tag, &edit.disc_number, ItemKey::DiscNumber, "disc_number", &mut out);
    // `OriginalReleaseDate` maps on all three primary tag types, and `extract_metadata` reads it
    // back with `s.get(..4)` — so a bare 4-digit year is the right shape.
    apply_number(tag, &edit.original_year, ItemKey::OriginalReleaseDate, "original_year", &mut out);

    apply_year(tag, &edit.year, &mut out);
    apply_bpm(tag, &edit.bpm, &mut out);
    apply_lyrics(tag, &edit.lyrics, &mut out);

    match edit.artwork {
        ArtworkEdit::Keep => {}
        ArtworkEdit::Remove => clear_front_cover(tag),
        ArtworkEdit::Replace => {
            // Clear only with a replacement in hand: `Replace` is a unit variant and the picture
            // travels beside the edit, so a caller *could* hand over `None` — and clearing first
            // would turn a Replace into a Remove across the whole batch.
            debug_assert!(picture.is_some(), "ArtworkEdit::Replace requires a Picture");
            if let Some(pic) = picture {
                clear_front_cover(tag);
                tag.push_picture(pic.clone());
            }
        }
    }

    UnsupportedFields(out)
}

/// Read-modify-write `path`'s tags in place. **Blocking** — callers go through `spawn_blocking`.
pub fn apply_to_file(
    path: &Path,
    edit: &TagEdit,
    picture: Option<&Picture>,
) -> Result<UnsupportedFields, AppError> {
    // `skip_artwork: false` is load-bearing rather than a default: skipping picture frames at
    // *parse* leaves `Tag.pictures` empty, and `save_to_path` writes that emptiness back over
    // every embedded picture the file had.
    let mut tagged = metadata::read_tags(path, false)?;

    let tag_type = tagged.primary_tag_type();

    // `insert_tag` no-ops when the `FileType` doesn't support the `TagType`, so without this
    // pre-flight the unsupported case surfaces as a confusing "no writable tag" below.
    if !tagged.tag_support(tag_type).is_writable() {
        return Err(AppError::metadata_msg(format!(
            "{tag_type:?} tags are read-only for {}",
            path.display()
        )));
    }

    if tagged.primary_tag_mut().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let Some(tag) = tagged.primary_tag_mut() else {
        return Err(AppError::metadata_msg(format!("no writable tag for {}", path.display())));
    };

    let unsupported = apply_edit(tag, edit, picture);

    tagged.save_to_path(path, WriteOptions::default()).map_err(|e| {
        AppError::metadata(format!("Failed to write tags to {}", path.display()), e)
    })?;

    Ok(unsupported)
}

/// Decode-validate a user-picked cover and produce an embeddable [`Picture`].
///
/// Two constraints that don't agree: lofty sniffs the mime from 8 bytes and rejects anything
/// outside PNG / JPEG / GIF / BMP / TIFF outright, while MP4's `covr` writer hard-errors on TIFF —
/// which lofty happily sniffs — but only on M4A/ALAC. No single picker filter can express that, so
/// normalize rather than filter: JPEG and PNG embed byte-for-byte, everything else is re-encoded
/// to JPEG, which every container accepts.
///
/// The decode is also the **validation**: `Picture::from_reader` never decodes, so a truncated
/// JPEG would embed into N files and only blow up at thumbnail time. Failing here aborts the batch
/// before any file is touched.
///
/// Call it **once per batch**, before any fan-out — it reads the image into memory, so per-track
/// would re-read the file N times.
pub fn cover_picture_from_path(path: &Path) -> Result<Picture, AppError> {
    let bytes = std::fs::read(path)
        .map_err(|e| AppError::metadata(format!("Failed to read cover {}", path.display()), e))?;

    let mut reader =
        image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|e| {
            AppError::metadata(format!("Unrecognized image format: {}", path.display()), e)
        })?;
    // The same bound every other artwork decode runs under. Reading from memory rather than a
    // path, this one can't go through `decode_capped` — but a forged header shouldn't get a
    // bigger allocation for being hand-picked.
    reader.limits(image_decode::capped_limits(image_decode::MAX_SOURCE_DIM));

    let format = reader.format();
    let decoded = reader
        .decode()
        .map_err(|e| AppError::metadata(format!("Failed to decode cover {}", path.display()), e))?;

    // Every container we target embeds JPEG and PNG as-is, so hand the original bytes through —
    // `decoded` was only ever the validator.
    let passthrough = matches!(format, Some(image::ImageFormat::Jpeg | image::ImageFormat::Png));

    let data = if passthrough {
        bytes
    } else {
        let mut jpeg = Vec::new();
        decoded.write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).map_err(|e| {
            AppError::metadata(format!("Failed to re-encode cover {} to JPEG", path.display()), e)
        })?;
        jpeg
    };

    let mut picture = Picture::from_reader(&mut Cursor::new(&data)).map_err(|e| {
        AppError::metadata(format!("Not a usable cover image: {}", path.display()), e)
    })?;
    // `from_reader` always yields `PictureType::Other`.
    picture.set_pic_type(PictureType::CoverFront);
    Ok(picture)
}

/// Read a file's lyrics tag for the single-selection Lyrics tab, `Lyrics`
/// falling back to `UnsyncLyrics` — the mirror of what [`apply_edit`] writes,
/// `ID3v2` having no `Lyrics` mapping. Blocking; call under `spawn_blocking`.
pub fn read_lyrics(path: &Path) -> Result<Option<String>, AppError> {
    let tagged = metadata::read_tags(path, true)?;
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(None);
    };
    Ok(tag
        .get_string(ItemKey::Lyrics)
        .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
        .map(str::to_owned)
        .filter(|s| !s.is_empty()))
}

#[cfg(test)]
#[path = "tests/tag_writer_tests.rs"]
mod tests;
