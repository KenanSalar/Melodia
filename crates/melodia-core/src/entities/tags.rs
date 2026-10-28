//! What a tag edit is, independent of writing one: the dialog builds it and
//! `media::ingest::tag_writer` consumes it, so the dialog names it without naming the writer.

use super::artist::ArtistCredit;
use super::credits::{CreditRole, ROLES, RoleCredits};
use super::genre::GenreList;

/// What the user did rather than what they left, because empty is not clear: writing `""` leaves
/// a tag our reader takes as `None` and other players display.
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
    /// Embed the picture the caller built with `media::ingest::cover_embed::cover_picture_from_path`.
    Replace,
}

/// Role credits and the roles the form could answer for, the only ones the writer clears. A batch
/// whose tracks disagree about a role shows nothing for it, and clearing that role would take a
/// credit the user was never shown from every file at once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleCreditEdit {
    credits: RoleCredits,
    answered: [bool; ROLES.len()],
}

impl RoleCreditEdit {
    /// `credits` scoped to the roles `answered` marks, positional over [`ROLES`].
    #[must_use]
    pub fn new(credits: RoleCredits, answered: [bool; ROLES.len()]) -> Self {
        Self { credits, answered }
    }

    /// Every role in scope, which is what a form showing one track always answers for.
    #[must_use]
    pub fn whole(credits: RoleCredits) -> Self {
        Self::new(credits, [true; ROLES.len()])
    }

    #[must_use]
    pub fn credits(&self) -> &RoleCredits {
        &self.credits
    }

    /// The roles the writer may touch, in [`ROLES`] order.
    pub fn scope(&self) -> impl Iterator<Item = CreditRole> + '_ {
        ROLES.into_iter().zip(self.answered).filter_map(|(role, ok)| ok.then_some(role))
    }

    #[must_use]
    pub fn answered(&self) -> [bool; ROLES.len()] {
        self.answered
    }
}

/// One dialog's worth of edits. Every field defaults to [`FieldEdit::Keep`], so a caller only sets
/// what the user actually changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagEdit {
    pub title: FieldEdit<String>,
    /// The whole credit, not a display string: the dialog edits names and join phrases separately
    /// and `media::ingest::tag_writer` needs both halves to write the pair of tags.
    pub artist: FieldEdit<ArtistCredit>,
    pub album_artist: FieldEdit<ArtistCredit>,
    pub album: FieldEdit<String>,
    /// The whole list, for [`Self::artist`]'s reason: the rendered string can't produce the
    /// repeated tag.
    pub genres: FieldEdit<GenreList>,
    /// Composer, conductor, producer and the rest of [`super::credits::ROLES`].
    pub credits: FieldEdit<RoleCreditEdit>,
    /// lofty's `Timestamp.year` is `u16`, so the form's year is parsed to `u16`.
    pub year: FieldEdit<u16>,
    pub original_year: FieldEdit<u16>,
    pub track_number: FieldEdit<u32>,
    pub track_total: FieldEdit<u32>,
    pub disc_number: FieldEdit<u32>,
    pub disc_total: FieldEdit<u32>,
    pub disc_subtitle: FieldEdit<String>,
    pub subtitle: FieldEdit<String>,
    pub comment: FieldEdit<String>,
    pub bpm: FieldEdit<f64>,
    pub initial_key: FieldEdit<String>,
    pub mood: FieldEdit<String>,
    pub grouping: FieldEdit<String>,
    pub work: FieldEdit<String>,
    pub movement: FieldEdit<String>,
    pub movement_number: FieldEdit<u32>,
    pub movement_total: FieldEdit<u32>,
    pub language: FieldEdit<String>,
    pub copyright: FieldEdit<String>,
    pub isrc: FieldEdit<String>,
    /// Release-level tags. Stored per-file like everything else here, and they reach `albums`
    /// through the re-ingest the commit ends with.
    pub label: FieldEdit<String>,
    pub catalog_number: FieldEdit<String>,
    pub barcode: FieldEdit<String>,
    pub media: FieldEdit<String>,
    pub release_type: FieldEdit<String>,
    pub release_country: FieldEdit<String>,
    pub compilation: FieldEdit<bool>,
    pub lyrics: FieldEdit<String>,
    /// Written by the rating write-back, not the Edit-Tags dialog. Stars, 0–5; `Clear` and
    /// `Set(0)` mean the same thing and both remove the tag.
    pub rating: FieldEdit<i32>,
    pub artwork: ArtworkEdit,
}

impl TagEdit {
    /// lofty rewrites a tag whether or not anything differs, so an open-then-Save would otherwise
    /// rewrite every file it covers.
    pub fn is_noop(&self) -> bool {
        self.rating == FieldEdit::Keep && self.no_field_but_rating()
    }

    /// The rating write-back's whole edit, which can neither re-home the track nor touch its
    /// artwork.
    pub fn is_rating_only(&self) -> bool {
        self.rating != FieldEdit::Keep && self.no_field_but_rating()
    }

    /// The FK-resolution key the commit builds from, less the folder, which no tag edit moves.
    pub fn moves_between_parents(&self) -> bool {
        self.artist != FieldEdit::Keep
            || self.album_artist != FieldEdit::Keep
            || self.album != FieldEdit::Keep
            || self.genres != FieldEdit::Keep
            || self.year != FieldEdit::Keep
    }

    /// The release-level fields this edit emptied, which the commit nulls after its upserts, since
    /// `upsert_album` coalesces a cleared NULL away, so it can raise one but never lower it.
    pub fn cleared_release_tags(&self) -> ClearedReleaseTags {
        ClearedReleaseTags {
            label: self.label == FieldEdit::Clear,
            catalog_number: self.catalog_number == FieldEdit::Clear,
            barcode: self.barcode == FieldEdit::Clear,
            media: self.media == FieldEdit::Clear,
            release_type: self.release_type == FieldEdit::Clear,
            release_country: self.release_country == FieldEdit::Clear,
            // A switch has no third state, so an un-ticked box arrives as `Set(false)`; the writer
            // treats that and `Clear` alike and removes the tag either way.
            compilation: matches!(self.compilation, FieldEdit::Clear | FieldEdit::Set(false)),
        }
    }

    /// Whether a tag a lyrics directory identifies a recording by moved (duration being out of an
    /// edit's reach), so the commit drops a stored miss earned under the old ones.
    pub fn renames_recording(&self) -> bool {
        self.title != FieldEdit::Keep
            || self.artist != FieldEdit::Keep
            || self.album != FieldEdit::Keep
    }

    /// Against the default rather than a term per field, so a field added later can't make
    /// [`Self::is_noop`] silently skip a real edit.
    fn no_field_but_rating(&self) -> bool {
        Self { rating: FieldEdit::Keep, ..self.clone() } == Self::default()
    }
}

/// Which release-level columns a commit has to null by hand, from
/// [`TagEdit::cleared_release_tags`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per clearable `albums` column, and the point is that the set is exhaustive"
)]
pub struct ClearedReleaseTags {
    pub label: bool,
    pub catalog_number: bool,
    pub barcode: bool,
    pub media: bool,
    pub release_type: bool,
    pub release_country: bool,
    pub compilation: bool,
}

impl ClearedReleaseTags {
    /// Nothing to null, which is every edit that emptied no release field.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// A field a write can fail on: a stable name for the log and a translated word for the toast,
/// which no one string can be. [`Self::Credit`] carries the role rather than ten more variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagField {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    TrackNumber,
    TrackTotal,
    DiscNumber,
    DiscTotal,
    OriginalYear,
    Bpm,
    Comment,
    Subtitle,
    DiscSubtitle,
    Grouping,
    Work,
    Movement,
    MovementNumber,
    MovementTotal,
    InitialKey,
    Mood,
    Language,
    Isrc,
    Copyright,
    Label,
    CatalogNumber,
    Barcode,
    Media,
    ReleaseType,
    ReleaseCountry,
    Compilation,
    Lyrics,
    Rating,
    Credit(CreditRole),
}

/// Every non-credit [`TagField`], in the order the UI's label list mirrors.
///
/// The order is the whole contract: [`TagField::label_index`] is a position in it, so a label list
/// that drifts names the wrong field and nothing else notices. `tags_tests` pins both halves.
pub const PLAIN_TAG_FIELDS: [TagField; 34] = [
    TagField::Title,
    TagField::Artist,
    TagField::Album,
    TagField::AlbumArtist,
    TagField::Genre,
    TagField::Year,
    TagField::TrackNumber,
    TagField::TrackTotal,
    TagField::DiscNumber,
    TagField::DiscTotal,
    TagField::OriginalYear,
    TagField::Bpm,
    TagField::Comment,
    TagField::Subtitle,
    TagField::DiscSubtitle,
    TagField::Grouping,
    TagField::Work,
    TagField::Movement,
    TagField::MovementNumber,
    TagField::MovementTotal,
    TagField::InitialKey,
    TagField::Mood,
    TagField::Language,
    TagField::Isrc,
    TagField::Copyright,
    TagField::Label,
    TagField::CatalogNumber,
    TagField::Barcode,
    TagField::Media,
    TagField::ReleaseType,
    TagField::ReleaseCountry,
    TagField::Compilation,
    TagField::Lyrics,
    TagField::Rating,
];

impl TagField {
    /// The stable name a log or a test spells. Never shown to a user, so it never moves.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::AlbumArtist => "album_artist",
            Self::Genre => "genre",
            Self::Year => "year",
            Self::TrackNumber => "track_number",
            Self::TrackTotal => "track_total",
            Self::DiscNumber => "disc_number",
            Self::DiscTotal => "disc_total",
            Self::OriginalYear => "original_year",
            Self::Bpm => "bpm",
            Self::Comment => "comment",
            Self::Subtitle => "subtitle",
            Self::DiscSubtitle => "disc_subtitle",
            Self::Grouping => "grouping",
            Self::Work => "work",
            Self::Movement => "movement",
            Self::MovementNumber => "movement_number",
            Self::MovementTotal => "movement_total",
            Self::InitialKey => "initial_key",
            Self::Mood => "mood",
            Self::Language => "language",
            Self::Isrc => "isrc",
            Self::Copyright => "copyright",
            Self::Label => "label",
            Self::CatalogNumber => "catalog_number",
            Self::Barcode => "barcode",
            Self::Media => "media",
            Self::ReleaseType => "release_type",
            Self::ReleaseCountry => "release_country",
            Self::Compilation => "compilation",
            Self::Lyrics => "lyrics",
            Self::Rating => "rating",
            Self::Credit(role) => role.as_db_str(),
        }
    }

    /// Where this field's label sits in the UI's list, credits following the plain fields.
    ///
    /// `None` only where a variant is missing from [`PLAIN_TAG_FIELDS`], which the tests forbid;
    /// the caller falls back to a message naming no field rather than to a wrong one.
    #[must_use]
    pub fn label_index(self) -> Option<usize> {
        match self {
            Self::Credit(role) => {
                ROLES.iter().position(|r| *r == role).map(|i| PLAIN_TAG_FIELDS.len() + i)
            }
            plain => PLAIN_TAG_FIELDS.iter().position(|f| *f == plain),
        }
    }
}

#[cfg(test)]
#[path = "tests/tags_tests.rs"]
mod tests;
