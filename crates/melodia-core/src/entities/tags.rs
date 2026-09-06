//! What a tag edit *is*, independent of writing one.
//!
//! The Edit-Tags dialog builds these out of its form and hands them to `library::tags`; the writer
//! in `media::ingest::tag_writer` consumes them. Neither end owns the vocabulary, which is why it sits
//! here — and why the dialog can name it without naming the writer.

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
            || self.genre != FieldEdit::Keep
            || self.year != FieldEdit::Keep
    }

    /// Every field except `rating` left at [`FieldEdit::Keep`].
    fn no_field_but_rating(&self) -> bool {
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
