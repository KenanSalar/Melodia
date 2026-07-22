//! Writing tags back to audio files (lofty).
//!
//! Melodia has always read tags and never written one. This is the write half:
//! a per-field tri-state edit (`Keep` / `Clear` / `Set`) applied to a file's
//! **primary** tag, plus the cover-art normalizer that turns a user-picked image
//! into something every container we ship can actually store.
//!
//! Nothing here touches the DB, the UI or `AppState`. [`apply_edit`] is pure —
//! it mutates an in-memory [`Tag`] and does no I/O at all — which is what makes
//! the per-format key tables below testable. [`apply_to_file`] is the blocking
//! read-modify-write around it; every caller goes through `spawn_blocking`.
//!
//! ## Why the primary tag, and only the primary tag
//!
//! [`TaggedFileExt::primary_tag_type`] is the format's canonical tag: `ID3v2` for
//! MP3 / WAV / AIFF / AAC, `VorbisComments` for FLAC / OGG, `Ilst` for M4A / ALAC.
//! Across the seven containers Melodia scans those three are the whole set, and
//! all three map every field this module exposes.
//!
//! We never fall back to `first_tag_mut()`. An MP3 carrying *only* an `ID3v1` tag
//! would then get the edit applied to that `ID3v1` tag, whose entire key set is
//! eight items — `AlbumArtist`, `Composer`, `Bpm`, `UnsyncLyrics` and
//! `OriginalReleaseDate` have no mapping at all, so half the user's edit would
//! vanish without a word. Creating a fresh primary tag instead also keeps the
//! writer aligned with the reader (`metadata.rs` reads
//! `primary_tag().or(first_tag())`), so the next `extract_metadata` reads back
//! exactly what we wrote.
//!
//! We also never [`Tag::re_map`] an existing tag into the primary type: `re_map`
//! deliberately **discards the format-specific companion tag** (it logs
//! "Discarding format-specific items due to remap"), which would throw away every
//! frame that has no `ItemKey` — `ReplayGain`, `MusicBrainz` ids, `POPM`, and so on.
//!
//! ## What survives an edit
//!
//! `GlobalOptions::preserve_format_specific_items` defaults to `true`: converting a
//! concrete `Id3v2Tag` into the generic [`Tag`] stashes a companion tag holding the
//! frames with no `ItemKey`, and merges them back on save. So `FieldEdit::Keep`
//! genuinely means keep, for fields this module has never heard of.
//!
//! `TaggedFile::save_to` writes *every* tag in the file, not just the one we
//! edited — it loops over all of them and re-serializes each. An MP3 with a
//! companion `ID3v1`, or a WAV with a RIFF INFO chunk, therefore keeps that tag,
//! rewritten unchanged and so now **stale** relative to the primary tag we just
//! edited. That is the honest trade, and it is why `WriteOptions::default()`'s
//! `remove_others: false` must stay: flipping it would strip those companion tags
//! outright, which is a bigger behavioural change than a stale `ID3v1`.

use std::io::Cursor;
use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::items::Timestamp;
use lofty::tag::{ItemValue, Tag, TagItem, TagType};

use crate::error::AppError;

/// Upper bound for a written BPM. Anything past this is a typo, not a tempo, and
/// a tag holding a 12-digit "tempo" is worse than one holding none.
const MAX_BPM: f64 = 1000.0;

/// A per-field tri-state. The dialog reports what the user *did*, not just the
/// value they left behind: an untouched field must not be rewritten, and an
/// emptied one must remove the key rather than write an empty string.
///
/// (Empty is not clear. `extract_metadata` filters whitespace-only tags to
/// `None`, so writing `""` would produce a ghost tag our own reader ignores but
/// other players happily display.)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FieldEdit<T> {
    /// User never touched it — leave the file's tag exactly as it is.
    #[default]
    Keep,
    /// User emptied it — remove the tag key entirely.
    Clear,
    /// User typed a value.
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

/// One dialog's worth of edits. Every field defaults to [`FieldEdit::Keep`], so
/// a caller only sets what the user actually changed.
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
    /// `MusicBrainz` Recording ID — written by the auto-tag backfill so `ListenBrainz`
    /// loves (which key on it) work; not surfaced in the Edit-Tags dialog.
    pub musicbrainz_track_id: FieldEdit<String>,
    pub bpm: FieldEdit<f64>,
    pub lyrics: FieldEdit<String>,
    pub artwork: ArtworkEdit,
}

impl TagEdit {
    /// True when the user changed nothing at all.
    ///
    /// The caller short-circuits on this. lofty rewrites the tag whether or not
    /// anything actually differs, so a reflexive open-then-Save on a 200-track
    /// album would otherwise rewrite 200 files — and, via the watcher, risk
    /// re-ingesting them — for nothing.
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

/// Fields the file's tag format has no key for.
///
/// Never an error — the rest of the edit still lands — but the user is told, so
/// "BPM didn't save" is a message and not a mystery.
///
/// In practice this is a safety net, not a routine outcome: all three primary tag
/// types map every field [`TagEdit`] exposes. Keep it (it is one bool check, and
/// it is what makes the BPM strategy correct), but don't build UI around it being
/// populated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnsupportedFields(pub Vec<&'static str>);

impl UnsupportedFields {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Write one text item, recording the field name when the key doesn't map.
///
/// **Every write goes through here.** [`Tag::insert_text`] re-maps the `ItemKey`
/// to the target `TagType` and returns `false` when no mapping exists — the item
/// is then silently dropped. That bool is what makes the BPM and lyrics fallbacks
/// below expressible.
///
/// This is also why the [`Accessor`] setters (`set_title`, `set_artist`, …) are
/// **not** used: they call `insert_text` and throw the bool away, and the trait's
/// default method bodies are empty no-ops, so a non-overriding impl would discard
/// the whole write.
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
/// Its own function because `ID3v2` stores this id in a binary `UFID` frame, not a
/// text frame: the generic [`Tag::insert_text`] support-check has no key mapping
/// for it and would refuse the write (dropping it into [`UnsupportedFields`]).
/// Inserting the item *unchecked* stores it regardless, and lofty's
/// `Tag → Id3v2Tag` conversion turns it into the `UFID` frame on save;
/// `VorbisComments` and MP4 map the key directly, so the unchecked path writes them
/// just the same. `insert_unchecked` replaces any existing item of this key.
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
/// MP4 has no picture-type concept: `Ilst` overwrites every picture's `pic_type`
/// with [`PictureType::Other`] on read. So keying the removal on `CoverFront`
/// alone plays out on an M4A as:
///
/// 1. `remove_picture_type(CoverFront)` matches nothing — the existing cover is
///    `Other`. **No-op.**
/// 2. The tag now holds `[old(Other), new(CoverFront)]`; both get written to `covr`.
/// 3. On the next read both come back as `Other`, and our own reader
///    (`CoverFront` > `CoverBack` > *first available*) falls through to first
///    available and picks up **the old cover**.
///
/// Replace would silently revert; Remove would do nothing at all. There is no
/// clear-all `Tag::remove_pictures()` in lofty 0.24, so the two-call form *is*
/// the idiom.
///
/// Collateral, and accepted: a FLAC that deliberately stores a non-cover image as
/// `Other` loses it on a Replace or Remove. `CoverBack`, booklet and artist
/// pictures survive. Melodia's data model has exactly one cover per track, so it
/// has nowhere to put the others anyway.
fn clear_front_cover(tag: &mut Tag) {
    tag.remove_picture_type(PictureType::CoverFront);
    tag.remove_picture_type(PictureType::Other);
}

/// BPM: the key differs per format, and `ItemKey::Bpm` **does not exist on `ID3v2`**.
///
/// | Format               | `ItemKey::Bpm`          | `ItemKey::IntegerBpm` |
/// |----------------------|-------------------------|-----------------------|
/// | Vorbis (FLAC / OGG)  | `BPM` ✓                 | —                     |
/// | `ID3v2` (MP3/WAV/AIFF) | **absent**            | `TBPM` ✓              |
/// | MP4 (`Ilst`)        | freeform `…iTunes:BPM`  | `tmpo` ✓              |
///
/// So `insert_text(Bpm, "128.5")` on an MP3 is a no-op returning `false`, and
/// `insert_text(IntegerBpm, "128")` on a FLAC is the same. Write `IntegerBpm`
/// (rounded) **always**, and additionally `Bpm` (the decimal) where it maps —
/// reporting BPM as unsupported only when *both* come back `false`. This is the
/// one field where the `insert_text` bool is genuinely load-bearing on a format
/// we ship.
fn apply_bpm(tag: &mut Tag, edit: &FieldEdit<f64>, out: &mut Vec<&'static str>) {
    match edit {
        FieldEdit::Keep => {}
        FieldEdit::Clear => {
            tag.remove_key(ItemKey::Bpm);
            tag.remove_key(ItemKey::IntegerBpm);
        }
        FieldEdit::Set(v) => {
            // Bound once, and write the *same* bounded value to both keys — the
            // integer and the decimal form of one BPM must not disagree.
            //
            // `f64::clamp` does NOT absorb NaN: it is `if self < min {…}
            // if self > max {…}`, and both comparisons are false for NaN, so NaN
            // passes straight through and `{:.0}` renders it as the literal
            // string "NaN". A dialog that parses its BPM field with
            // `str::parse::<f64>()` accepts "nan" and "inf", so guard it here.
            //
            // Format the rounded value straight to a string rather than casting
            // through an integer: a float→int `as` would trip
            // `cast_possible_truncation` / `cast_sign_loss` under the pedantic
            // gate, and the tag only ever wants the decimal text anyway.
            //
            // `.round()` before formatting is load-bearing: `{:.0}` rounds
            // half-to-even, so it would render 128.5 as "128". `f64::round` is
            // half-away-from-zero, which is what "rounded BPM" means to everyone
            // else — and once the value is integral, `{:.0}` is exact.
            let bpm = if v.is_nan() { 0.0 } else { v.clamp(0.0, MAX_BPM) };
            let int_ok = tag.insert_text(ItemKey::IntegerBpm, format!("{:.0}", bpm.round()));
            let dec_ok = tag.insert_text(ItemKey::Bpm, bpm.to_string());
            if !int_ok && !dec_ok {
                out.push("bpm");
            }
        }
    }
}

/// Lyrics: `ItemKey::Lyrics` still exists — it's **`ID3v2`** that lacks the mapping.
///
/// | Format   | `ItemKey::Lyrics`        | `ItemKey::UnsyncLyrics` |
/// |----------|--------------------------|-------------------------|
/// | Vorbis   | `LYRICS` ✓               | `UNSYNCEDLYRICS` ✓      |
/// | `ID3v2`  | **absent** (overloaded across SYLT/USLT) | `USLT` ✓ |
/// | MP4      | `©lyr` ✓                 | `©lyr` ✓                |
///
/// `LYRICS` is the key Picard / `foobar2000` / `MusicBee` actually write in Vorbis
/// comments. Writing only `UnsyncLyrics` would put our FLAC lyrics under
/// `UNSYNCEDLYRICS`, where **no other player looks** — and theirs would be
/// invisible to us. So write keyed by tag type, and clear *both*.
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

/// Year, done by hand.
///
/// lofty's [`Accessor`] exposes `date: Timestamp`, not `year`, and `set_date`
/// discards the `insert_text` bool. So do what `set_date` does — `remove_key(Year)`
/// + `insert_text(RecordingDate, ts)` — with the bool visible.
///
/// Seed from the existing `tag.date()` so editing only the year **preserves an
/// existing month/day**. `Timestamp`'s `Display` writes `{:04}` for the year and
/// appends `-MM-DD` only when those parts are present, so a year-only edit renders
/// as `"2024"`.
fn apply_year(tag: &mut Tag, edit: &FieldEdit<u16>, out: &mut Vec<&'static str>) {
    match edit {
        FieldEdit::Keep => {}
        // Removes both `Year` and `RecordingDate`.
        FieldEdit::Clear => tag.remove_date(),
        FieldEdit::Set(y) => {
            let existing = tag.date().unwrap_or_default();
            let ts = Timestamp { year: *y, ..existing };
            tag.remove_key(ItemKey::Year);
            set_text(tag, ItemKey::RecordingDate, ts.to_string(), "year", out);
        }
    }
}

/// Apply `edit` to an in-memory tag. **Pure — no I/O at all**; the `Replace`
/// image is decoded by the caller and handed in already built.
///
/// Returns the fields this tag format had no key for.
pub fn apply_edit(tag: &mut Tag, edit: &TagEdit, picture: Option<&Picture>) -> UnsupportedFields {
    let mut out: Vec<&'static str> = Vec::new();

    apply_string(tag, &edit.title, ItemKey::TrackTitle, "title", &mut out);
    apply_string(tag, &edit.artist, ItemKey::TrackArtist, "artist", &mut out);
    apply_string(
        tag,
        &edit.album_artist,
        ItemKey::AlbumArtist,
        "album_artist",
        &mut out,
    );
    apply_string(tag, &edit.album, ItemKey::AlbumTitle, "album", &mut out);
    apply_string(tag, &edit.genre, ItemKey::Genre, "genre", &mut out);
    apply_string(tag, &edit.composer, ItemKey::Composer, "composer", &mut out);
    apply_string(tag, &edit.comment, ItemKey::Comment, "comment", &mut out);
    apply_recording_id(tag, &edit.musicbrainz_track_id);

    apply_number(
        tag,
        &edit.track_number,
        ItemKey::TrackNumber,
        "track_number",
        &mut out,
    );
    apply_number(
        tag,
        &edit.disc_number,
        ItemKey::DiscNumber,
        "disc_number",
        &mut out,
    );
    // `OriginalReleaseDate` maps on all three primary tag types (Vorbis
    // `ORIGINALDATE`, ID3v2 `TDOR`, MP4 freeform `…iTunes:ORIGINALDATE`), and
    // `extract_metadata` reads it back with `s.get(..4)`, so the bare 4-digit year
    // is the right shape.
    apply_number(
        tag,
        &edit.original_year,
        ItemKey::OriginalReleaseDate,
        "original_year",
        &mut out,
    );

    apply_year(tag, &edit.year, &mut out);
    apply_bpm(tag, &edit.bpm, &mut out);
    apply_lyrics(tag, &edit.lyrics, &mut out);

    match edit.artwork {
        ArtworkEdit::Keep => {}
        ArtworkEdit::Remove => clear_front_cover(tag),
        ArtworkEdit::Replace => {
            // Clear only with a replacement in hand. `Replace` is a unit variant
            // (the orchestrator owns the picked `PathBuf`, which it needs for
            // `cache_image_file` anyway), so the picture travels beside the edit
            // and a caller *could* hand us `None` — and clearing first would
            // silently turn a Replace into a Remove across the whole batch.
            debug_assert!(picture.is_some(), "ArtworkEdit::Replace requires a Picture");
            if let Some(pic) = picture {
                clear_front_cover(tag);
                tag.push_picture(pic.clone());
            }
        }
    }

    UnsupportedFields(out)
}

/// Read-modify-write `path`'s tags in place. **Blocking** — callers go through
/// `spawn_blocking`.
pub fn apply_to_file(
    path: &Path,
    edit: &TagEdit,
    picture: Option<&Picture>,
) -> Result<UnsupportedFields, AppError> {
    // Default `ParseOptions` — `read_cover_art: true`. NEVER reuse
    // `extract_metadata`'s `skip_artwork` branch (`read_cover_art(false)`) here:
    // that skips picture frames *at parse*, and pictures live in `Tag.pictures`,
    // not in the format-specific companion. Skipped at read ⇒ absent from the tag
    // ⇒ `save_to_path` would **silently delete every embedded picture**.
    let mut tagged = lofty::probe::read_from_path(path)
        .map_err(|e| AppError::metadata(format!("Failed to read tags from {}", path.display()), e))?;

    let tag_type = tagged.primary_tag_type();

    // Honest pre-flight: `insert_tag` NO-OPS and returns `None` when the FileType
    // doesn't support the TagType, so without this the unsupported case would only
    // surface as a confusing "no writable tag" below.
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
        return Err(AppError::metadata_msg(format!(
            "no writable tag for {}",
            path.display()
        )));
    };

    let unsupported = apply_edit(tag, edit, picture);

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| AppError::metadata(format!("Failed to write tags to {}", path.display()), e))?;

    Ok(unsupported)
}

/// Decode-validate a user-picked cover and produce an embeddable [`Picture`].
///
/// Two constraints have to be satisfied at once, and they don't agree:
///
/// - **lofty** sniffs the mime from the first 8 bytes and recognises only
///   PNG / JPEG / GIF / BMP / TIFF. Anything else — WebP included — is rejected
///   outright as `NotAPicture`; it does *not* fall through to an unknown mime.
/// - **MP4's `covr` writer** accepts only Gif / Jpeg / Png / Bmp, and returns
///   `FileEncodingError` on anything else. So a TIFF that lofty happily sniffs
///   would still hard-error the save — but only on M4A/ALAC. No single picker
///   filter can express that difference.
///
/// So normalize rather than filter. JPEG and PNG (what the picker offers alongside
/// WebP) embed byte-for-byte, with no lossy re-compression; everything else is
/// re-encoded to JPEG, which every container accepts.
///
/// The decode is also the **validation** step. `Picture::from_reader` only sniffs
/// 8 bytes and never decodes, so a truncated JPEG would otherwise embed happily
/// into N files and only blow up later at thumbnail time. Failing here aborts the
/// whole edit before any file is touched.
///
/// Call this **once per batch**, before any fan-out: it reads the image into
/// memory, so building it per-track would re-read the file N times.
pub fn cover_picture_from_path(path: &Path) -> Result<Picture, AppError> {
    let bytes = std::fs::read(path)
        .map_err(|e| AppError::metadata(format!("Failed to read cover {}", path.display()), e))?;

    let reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| {
            AppError::metadata(format!("Unrecognized image format: {}", path.display()), e)
        })?;

    let format = reader.format();
    let decoded = reader.decode().map_err(|e| {
        AppError::metadata(format!("Failed to decode cover {}", path.display()), e)
    })?;

    // JPEG and PNG are embeddable as-is by every container we target, so hand the
    // original bytes through untouched. `decoded` was only ever the validator.
    let passthrough = matches!(
        format,
        Some(image::ImageFormat::Jpeg | image::ImageFormat::Png)
    );

    let data = if passthrough {
        bytes
    } else {
        let mut jpeg = Vec::new();
        decoded
            .write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .map_err(|e| {
                AppError::metadata(
                    format!("Failed to re-encode cover {} to JPEG", path.display()),
                    e,
                )
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

/// Read a file's lyrics tag for the single-selection Lyrics tab: `Lyrics`
/// (`VorbisComments` / MP4) falling back to `UnsyncLyrics` (`ID3v2` / MP4).
///
/// `ID3v2` has no `Lyrics` mapping (lofty overloads it across `SYLT` / `USLT`),
/// so an MP3's lyrics only surface through `UnsyncLyrics` (`USLT`) — the mirror
/// of what [`apply_edit`] writes. Blocking I/O; call under `spawn_blocking`.
pub fn read_lyrics(path: &Path) -> Result<Option<String>, AppError> {
    let tagged = lofty::probe::read_from_path(path).map_err(|e| {
        AppError::metadata(format!("Failed to read tags from {}", path.display()), e)
    })?;
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
