//! What a tag edit *is*, independent of writing one.
//!
//! The Edit-Tags dialog builds these out of its form and hands them to `library::tags`; the writer
//! in `media::ingest::tag_writer` consumes them. Neither end owns the vocabulary, which is why it sits
//! here — and why the dialog can name it without naming the writer.

use super::artist::ArtistCredit;
use super::credits::{CreditRole, ROLES, RoleCredits};
use super::genre::GenreList;

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
    /// Embed the picture the caller built with `media::ingest::tag_writer::cover_picture_from_path`.
    Replace,
}

/// Role credits to write, and the roles the form behind them could answer for.
///
/// The writer clears every role it is handed before writing, which is what makes an emptied box
/// actually leave the file. That only holds where the form spoke for all ten, and a batch
/// selection whose tracks disagree about a role shows nothing for it — so the set rebuilt from
/// such a form names nobody in that role, and clearing it would take a credit the user was never
/// shown from every file at once.
///
/// The scope is what the form could answer for: every role on one track, and on a selection the
/// roles it agreed on plus any the user has since filled in. The writer touches nothing outside
/// it. Kept beside the credits so the scope travels with the form it describes, where the writer
/// used to assume all ten.
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
    /// The whole list, for [`Self::artist`]'s reason: the `genre` column is the rendered form of
    /// these and `track_genres` the rows, and a writer handed only the string could not produce
    /// the repeated tag that carries more than one.
    pub genres: FieldEdit<GenreList>,
    /// Composer, conductor, producer and the rest — the roles of
    /// [`super::credits::ROLES`], as one set, plus the roles the writer may touch.
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
    /// Written by the auto-tag backfill so `ListenBrainz` loves — which key on it — work. Not
    /// surfaced in the Edit-Tags dialog.
    pub musicbrainz_track_id: FieldEdit<String>,
    pub lyrics: FieldEdit<String>,
    /// Written by the rating write-back, not the Edit-Tags dialog. Stars, 0–5; `Clear` and
    /// `Set(0)` mean the same thing and both remove the tag.
    pub rating: FieldEdit<i32>,
    pub artwork: ArtworkEdit,
}

impl TagEdit {
    /// True when the user changed nothing at all; `library::tags`' writer short-circuits on it.
    /// lofty rewrites the tag whether or not anything differs, so a reflexive open-then-Save on a
    /// 200-track album would otherwise rewrite 200 files, and through the watcher risk
    /// re-ingesting them.
    pub fn is_noop(&self) -> bool {
        self.rating == FieldEdit::Keep && self.no_field_but_rating()
    }

    /// True when the rating is the only thing being written, which is the whole of what
    /// `library::ratings`'s write-back sends. Such an edit can neither re-home the track
    /// nor touch its artwork, so the commit skips the work that answers to both.
    pub fn is_rating_only(&self) -> bool {
        self.rating != FieldEdit::Keep && self.no_field_but_rating()
    }

    /// Whether this edit can move the track to a different album, artist or genre. These five are
    /// the FK-resolution key `library::tags`'s commit is built from, minus the folder, which comes
    /// off the path and so no tag edit can move.
    pub fn moves_between_parents(&self) -> bool {
        self.artist != FieldEdit::Keep
            || self.album_artist != FieldEdit::Keep
            || self.album != FieldEdit::Keep
            || self.genres != FieldEdit::Keep
            || self.year != FieldEdit::Keep
    }

    /// The release-level fields this edit **emptied**.
    ///
    /// They live on `albums`, which every track of a release writes through a
    /// `COALESCE(excluded.x, albums.x)` upsert: a cleared field arrives as the NULL that coalesce
    /// discards, so the stored value would otherwise survive the tag leaving the file and the
    /// release would go on showing a label nothing carries. The compilation flag is worse still,
    /// its upsert being an `OR` that no re-ingest can ever bring back down.
    ///
    /// `library::tags`' commit answers this once and nulls what it names, after the upserts.
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

    /// Whether this edit moves any tag a lyrics directory identifies a recording by.
    ///
    /// Its signature takes four fields and duration is the one no tag edit can reach, so the album
    /// counts alongside the two that name the song. `library::tags`' commit drops the stored sheet
    /// on a true answer: a cached miss was earned under tags the file no longer carries, and it
    /// stands for a month.
    pub fn renames_recording(&self) -> bool {
        self.title != FieldEdit::Keep
            || self.artist != FieldEdit::Keep
            || self.album != FieldEdit::Keep
    }

    /// Every field except `rating` left at [`FieldEdit::Keep`].
    ///
    /// A comparison against the default rather than a term per field, because the failure mode of
    /// the chain this replaced is silent and costly: a field missing from it makes
    /// [`Self::is_noop`] answer true for a real edit, and the commit then skips the write with no
    /// error anywhere. Every field defaults to `Keep`, so one `==` covers the ones added next.
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

#[cfg(test)]
#[path = "tests/tags_tests.rs"]
mod tests;
