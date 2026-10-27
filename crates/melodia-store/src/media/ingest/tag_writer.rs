//! Writing tags back to audio files: a per-field `Keep` / `Clear` / `Set` edit applied to a file's
//! **primary** tag. [`apply_edit`] is pure, which is what makes the per-format key mappings
//! testable, and [`apply_to_file`] is the blocking read-modify-write around it. The picture an
//! edit carries is built by [`super::cover_embed`].
//!
//! The primary tag is `ID3v2`, `VorbisComments` or `Ilst` across everything lofty can tag here, and
//! all three map every field this module exposes. **Never `first_tag_mut()`**: on an ID3v1-only MP3
//! it would drop album artist, composer, BPM and lyrics, where a fresh primary tag is also what the
//! reader's `primary_tag().or(first_tag())` reads back. **Never [`Tag::re_map`]** either: it
//! discards the companion tag holding every frame with no `ItemKey` (`ReplayGain`, `MusicBrainz`
//! ids, `POPM`), which `preserve_format_specific_items` otherwise merges back on save, so `Keep`
//! means keep for fields this module never heard of.
//!
//! `save_to` re-serializes *every* tag, so an MP3's `ID3v1` or a WAV's RIFF INFO survives unchanged
//! and stale against the primary. `WriteOptions`' `remove_others: false` must stay: flipping it
//! strips those tags outright.

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::prelude::ItemKey;
use lofty::tag::items::Timestamp;
use lofty::tag::{ItemValue, Tag, TagItem, TagType};

use super::{metadata, rating_tags, role_tags};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::tags::{ArtworkEdit, FieldEdit, RoleCreditEdit, TagEdit, TagField};
use melodia_core::error::AppError;
use melodia_core::utils::atomic_file;

/// The highest BPM written; past it is a typo, not a tempo.
const MAX_BPM: f64 = 1000.0;

/// What a write left unwritten, and the container that would not take it, named as its users know
/// it, which [`apply_edit`], handed only a `Tag`, can't say.
///
/// A safety net, never an error: all three primary tag types map every field [`TagEdit`] exposes,
/// so don't build UI around `unsupported` being populated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteOutcome {
    /// A proper noun, so nothing translates it.
    pub format: Option<&'static str>,
    pub unsupported: Vec<TagField>,
}

/// The clause a log line ends with for fields a container had no key for, the subject in front of
/// it being the caller's.
#[must_use]
pub fn describe_unsupported(format: Option<&str>, fields: &[TagField]) -> String {
    let named: Vec<&str> = fields.iter().copied().map(TagField::as_db_str).collect();
    format!("{} has no key for {}", format.unwrap_or("this container"), named.join(", "))
}

/// What to call `file_type` when telling someone their file wouldn't take a field: the extension
/// on the file they picked, not lofty's `Mpeg` or `Mp4`.
///
/// `None` rather than a filler word, which would read as English inside a translated sentence.
fn format_name(file_type: FileType) -> Option<&'static str> {
    Some(match file_type {
        FileType::Aac => "AAC",
        FileType::Aiff => "AIFF",
        FileType::Ape => "APE",
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MP3",
        FileType::Mp4 => "M4A",
        FileType::Mpc => "Musepack",
        FileType::Opus => "Opus",
        FileType::Vorbis => "Ogg Vorbis",
        FileType::Speex => "Speex",
        FileType::Wav => "WAV",
        FileType::WavPack => "WavPack",
        FileType::Custom(name) => name,
        _ => return None,
    })
}

/// One tag mid-edit, and the fields it turned out to have no key for. [`Self::finish`] is the only
/// way to the list, so a half-applied edit can't be reported.
struct TagWrite<'a> {
    tag: &'a mut Tag,
    unsupported: Vec<TagField>,
}

impl<'a> TagWrite<'a> {
    fn new(tag: &'a mut Tag) -> Self {
        Self { tag, unsupported: Vec::new() }
    }

    fn finish(self) -> Vec<TagField> {
        self.unsupported
    }

    /// Writes one text item, recording `field` when the key doesn't map.
    ///
    /// **Every single-valued write goes through here**: [`Tag::insert_text`] drops an unmapped key
    /// silently and says so only in its bool, which the `Accessor` setters throw away.
    fn text(&mut self, key: ItemKey, value: String, field: TagField) {
        if !self.tag.insert_text(key, value) {
            self.unsupported.push(field);
        }
    }

    fn string(&mut self, edit: &FieldEdit<String>, key: ItemKey, field: TagField) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => self.tag.remove_key(key),
            FieldEdit::Set(v) => self.text(key, v.clone(), field),
        }
    }

    fn number<T: std::fmt::Display>(&mut self, edit: &FieldEdit<T>, key: ItemKey, field: TagField) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => self.tag.remove_key(key),
            FieldEdit::Set(v) => self.text(key, v.to_string(), field),
        }
    }

    /// Writes an artist field: the printed credit under `printed_key`, one value per name under
    /// `list_key`. A one-name credit writes **no** list, its absence being what says the string is
    /// one artist.
    fn credits(
        &mut self,
        edit: &FieldEdit<ArtistCredit>,
        printed_key: ItemKey,
        list_key: ItemKey,
        field: TagField,
    ) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => {
                self.tag.remove_key(printed_key);
                self.tag.remove_key(list_key);
            }
            FieldEdit::Set(credit) => {
                let printed_ok =
                    self.tag.insert_text(printed_key, credit.line().unwrap_or_default().to_owned());
                let listed_ok = if credit.artists().len() > 1 {
                    set_texts(self.tag, list_key, credit.artists().iter().map(|a| a.name.clone()))
                } else {
                    self.tag.remove_key(list_key);
                    true
                };
                if !(printed_ok && listed_ok) {
                    self.unsupported.push(field);
                }
            }
        }
    }

    /// Writes one value per genre, `GENRE` *being* the list. How lofty stores them is per-format,
    /// and the `;`-joined single value `metadata::read_genres` also accepts is other taggers' shape,
    /// never written here.
    fn genres(&mut self, edit: &FieldEdit<GenreList>) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => self.tag.remove_key(ItemKey::Genre),
            FieldEdit::Set(genres) => {
                if !set_texts(self.tag, ItemKey::Genre, genres.names().iter().cloned()) {
                    self.unsupported.push(TagField::Genre);
                }
            }
        }
    }

    /// Writes the role credits, reporting the roles this format has no key for. What a scope clears
    /// is argued at [`RoleCreditEdit`], the per-format holes in [`super::role_tags`].
    ///
    /// The dialog sends an emptied form as a `Set` of an empty set, so `Clear` is for a producer with
    /// no scope to offer.
    fn roles(&mut self, edit: &FieldEdit<RoleCreditEdit>) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => role_tags::clear(self.tag),
            FieldEdit::Set(credits) => {
                for role in role_tags::write_roles(self.tag, credits) {
                    self.unsupported.push(TagField::Credit(role));
                }
            }
        }
    }

    /// Writes a flag as `1`. `Set(false)` removes rather than writing `0`, which a player checking
    /// only for the key's presence reads as set.
    fn flag(&mut self, edit: &FieldEdit<bool>, key: ItemKey, field: TagField) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear | FieldEdit::Set(false) => self.tag.remove_key(key),
            FieldEdit::Set(true) => self.text(key, "1".to_owned(), field),
        }
    }

    /// `ItemKey::Bpm` has no `ID3v2` mapping and `IntegerBpm` no Vorbis one, so both are written
    /// and BPM is unsupported only when *both* refuse.
    fn bpm(&mut self, edit: &FieldEdit<f64>) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => {
                self.tag.remove_key(ItemKey::Bpm);
                self.tag.remove_key(ItemKey::IntegerBpm);
            }
            FieldEdit::Set(v) => {
                // One bound value for both keys. `clamp` passes through the NaN `parse::<f64>()`
                // accepts, and `.round()` is owed because `{:.0}` rounds half to even.
                let bpm = if v.is_nan() { 0.0 } else { v.clamp(0.0, MAX_BPM) };
                let int_ok =
                    self.tag.insert_text(ItemKey::IntegerBpm, format!("{:.0}", bpm.round()));
                let dec_ok = self.tag.insert_text(ItemKey::Bpm, bpm.to_string());
                if !int_ok && !dec_ok {
                    self.unsupported.push(TagField::Bpm);
                }
            }
        }
    }

    /// `ID3v2` has no `ItemKey::Lyrics` mapping. Vorbis comments take `LYRICS`, the key other
    /// taggers write and read there, where `UNSYNCEDLYRICS` would hide ours from them and theirs
    /// from us. Clear both.
    fn lyrics(&mut self, edit: &FieldEdit<String>) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => {
                self.tag.remove_key(ItemKey::Lyrics);
                self.tag.remove_key(ItemKey::UnsyncLyrics);
            }
            FieldEdit::Set(v) => {
                let key = if self.tag.tag_type() == TagType::VorbisComments {
                    ItemKey::Lyrics
                } else {
                    ItemKey::UnsyncLyrics
                };
                self.text(key, v.clone(), TagField::Lyrics);
            }
        }
    }

    /// Which key holds a rating, and on what scale, is per-format and argued in [`rating_tags`].
    /// `Set(0)` folds into the clear there, unrated and zero stars being the same thing.
    fn rating(&mut self, edit: &FieldEdit<i32>) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => rating_tags::clear(self.tag),
            FieldEdit::Set(stars) => {
                if !rating_tags::write_stars(self.tag, *stars) {
                    self.unsupported.push(TagField::Rating);
                }
            }
        }
    }

    /// Year, by hand: `Accessor::set_date` discards the `insert_text` bool, and `remove_date`
    /// reaches two of the three keys the reader takes a year from.
    ///
    /// **Every key goes, then `RecordingDate` is written**: the reader prefers `ReleaseDate`, so a
    /// stale one would read back over the edit, and `TDRC`/`DATE` is the key other players read.
    /// Seeding from the existing timestamp keeps a month and day through a year-only edit.
    fn year(&mut self, edit: &FieldEdit<u16>) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => clear_release_dates(self.tag),
            FieldEdit::Set(y) => {
                let existing = metadata::release_timestamp(Some(self.tag)).unwrap_or_default();
                let ts = Timestamp { year: *y, ..existing };
                clear_release_dates(self.tag);
                self.text(ItemKey::RecordingDate, ts.to_string(), TagField::Year);
            }
        }
    }

    /// Reports nothing: a container that refuses a picture says so at save time.
    fn artwork(&mut self, edit: &ArtworkEdit, picture: Option<&Picture>) {
        match edit {
            ArtworkEdit::Keep => {}
            ArtworkEdit::Remove => clear_front_cover(self.tag),
            ArtworkEdit::Replace => {
                // Clear only with a picture in hand, or a caller passing `None` turns a Replace
                // into a Remove across the whole batch.
                debug_assert!(picture.is_some(), "ArtworkEdit::Replace requires a Picture");
                if let Some(pic) = picture {
                    clear_front_cover(self.tag);
                    self.tag.push_picture(pic.clone());
                }
            }
        }
    }
}

/// Replaces every value under `key`, where [`Tag::insert_text`] keeps exactly one. Returns whether
/// the key maps rather than recording it, since [`TagWrite::credits`] writes two keys and reports
/// the field once.
fn set_texts(tag: &mut Tag, key: ItemKey, values: impl IntoIterator<Item = String>) -> bool {
    tag.remove_key(key);
    let mut supported = true;
    for value in values {
        supported &= tag.push(TagItem::new(key, ItemValue::Text(value)));
    }
    supported
}

/// Removes the front cover, **both** `CoverFront` and `Other`: `Ilst` flattens every picture type
/// to `Other` on read, so `CoverFront` alone matches nothing on an M4A and the reader falls through
/// to the *old* cover. Costs a FLAC any non-cover image it stored as `Other`.
fn clear_front_cover(tag: &mut Tag) {
    tag.remove_picture_type(PictureType::CoverFront);
    tag.remove_picture_type(PictureType::Other);
}

fn clear_release_dates(tag: &mut Tag) {
    for key in metadata::RELEASE_DATE_KEYS {
        tag.remove_key(key);
    }
}

/// Applies `edit` to an in-memory tag, returning the fields its format had no key for. Pure: the
/// `Replace` picture arrives already built.
pub fn apply_edit(tag: &mut Tag, edit: &TagEdit, picture: Option<&Picture>) -> Vec<TagField> {
    let mut write = TagWrite::new(tag);

    write.string(&edit.title, ItemKey::TrackTitle, TagField::Title);
    write.credits(&edit.artist, ItemKey::TrackArtist, ItemKey::TrackArtists, TagField::Artist);
    write.credits(
        &edit.album_artist,
        ItemKey::AlbumArtist,
        ItemKey::AlbumArtists,
        TagField::AlbumArtist,
    );
    write.string(&edit.album, ItemKey::AlbumTitle, TagField::Album);
    write.genres(&edit.genres);
    write.roles(&edit.credits);
    write.string(&edit.comment, ItemKey::Comment, TagField::Comment);
    write.string(&edit.disc_subtitle, ItemKey::SetSubtitle, TagField::DiscSubtitle);
    write.string(&edit.subtitle, ItemKey::TrackSubtitle, TagField::Subtitle);
    write.string(&edit.initial_key, ItemKey::InitialKey, TagField::InitialKey);
    write.string(&edit.mood, ItemKey::Mood, TagField::Mood);
    write.string(&edit.grouping, ItemKey::ContentGroup, TagField::Grouping);
    write.string(&edit.work, ItemKey::Work, TagField::Work);
    write.string(&edit.movement, ItemKey::Movement, TagField::Movement);
    write.string(&edit.language, ItemKey::Language, TagField::Language);
    write.string(&edit.copyright, ItemKey::CopyrightMessage, TagField::Copyright);
    write.string(&edit.isrc, ItemKey::Isrc, TagField::Isrc);
    write.string(&edit.label, ItemKey::Label, TagField::Label);
    write.string(&edit.catalog_number, ItemKey::CatalogNumber, TagField::CatalogNumber);
    write.string(&edit.barcode, ItemKey::Barcode, TagField::Barcode);
    write.string(&edit.media, ItemKey::OriginalMediaType, TagField::Media);
    write.string(&edit.release_type, ItemKey::MusicBrainzReleaseType, TagField::ReleaseType);
    write.string(&edit.release_country, ItemKey::ReleaseCountry, TagField::ReleaseCountry);

    write.number(&edit.track_number, ItemKey::TrackNumber, TagField::TrackNumber);
    write.number(&edit.track_total, ItemKey::TrackTotal, TagField::TrackTotal);
    write.number(&edit.disc_number, ItemKey::DiscNumber, TagField::DiscNumber);
    write.number(&edit.disc_total, ItemKey::DiscTotal, TagField::DiscTotal);
    write.number(&edit.movement_number, ItemKey::MovementNumber, TagField::MovementNumber);
    write.number(&edit.movement_total, ItemKey::MovementTotal, TagField::MovementTotal);
    // A bare year is a whole timestamp, under a key all three primary tag types map.
    write.number(&edit.original_year, ItemKey::OriginalReleaseDate, TagField::OriginalYear);

    write.year(&edit.year);
    write.bpm(&edit.bpm);
    write.flag(&edit.compilation, ItemKey::FlagCompilation, TagField::Compilation);
    write.lyrics(&edit.lyrics);
    write.rating(&edit.rating);
    write.artwork(&edit.artwork, picture);

    write.finish()
}

/// Applies `edit` to the file at `path`. Blocking.
pub fn apply_to_file(
    path: &Path,
    edit: &TagEdit,
    picture: Option<&Picture>,
) -> Result<WriteOutcome, AppError> {
    // `Full`, or the pictures skipped at parse are saved back as none.
    let mut tagged = metadata::read_tags(path, metadata::TagScope::Full)?;

    let format = format_name(tagged.file_type());
    let tag_type = tagged.primary_tag_type();

    // `insert_tag` no-ops on a `TagType` the file can't hold, which would surface below as a
    // confusing "no writable tag".
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

    // Not in place: a tag edit routinely targets the track a deck holds open.
    atomic_file::rewrite_with_sync(path, |target| {
        tagged.save_to_path(target, WriteOptions::default()).map_err(|e| {
            AppError::metadata(format!("Failed to write tags to {}", path.display()), e)
        })
    })?;

    Ok(WriteOutcome { format, unsupported })
}

/// Reads a file's lyrics tag, `Lyrics` before `UnsyncLyrics` as [`apply_edit`] writes them.
/// Blocking.
pub fn read_lyrics(path: &Path) -> Result<Option<String>, AppError> {
    let tagged = metadata::read_tags(path, metadata::TagScope::TagsOnly)?;
    Ok(tagged.primary_tag().or_else(|| tagged.first_tag()).and_then(lyrics_from))
}

/// The lyrics and both artist credits off one parse, for the Edit-Tags dialog's single-file open.
/// Blocking.
pub fn read_lyrics_and_credits(
    path: &Path,
) -> Result<(Option<String>, (ArtistCredit, ArtistCredit)), AppError> {
    let tagged = metadata::read_tags(path, metadata::TagScope::TagsOnly)?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    Ok((tag.and_then(lyrics_from), metadata::credits_from_tag(tag)))
}

fn lyrics_from(tag: &Tag) -> Option<String> {
    tag.get_string(ItemKey::Lyrics)
        .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
#[path = "tests/tag_writer_tests.rs"]
mod tests;
