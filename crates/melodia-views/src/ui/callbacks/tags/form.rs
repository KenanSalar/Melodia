//! What the dialog's form holds, and how it is read back off the globals.
//!
//! Two shapes, and the split is the contract: the text fields cross the boundary as strings and
//! diff as strings, while the four multi-value fields are edited as rows and diff structurally.

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::tags::RoleCreditEdit;
use melodia_ui::TagEditor;

/// Which credit a `TagEditor` row callback names, mirroring the global's own `field-artist` /
/// `field-album-artist`. Two spellings of one position is what drifts, so the Slint side reads
/// its from the global rather than restating the number.
pub(super) const CREDIT_ARTIST: usize = 0;
pub(super) const CREDIT_ALBUM_ARTIST: usize = 1;
pub(super) const CREDIT_FIELD_COUNT: usize = 2;

/// Every editable field that crosses the boundary as a string, named rather than numbered.
///
/// This replaced three positional lists — a getter array, `populate`'s `field!` calls and the
/// diff — that had to agree on an index per field. At eleven fields that was merely fragile; the
/// vocabulary is thirty-odd now, and the failure mode is silent and destructive: an index off by
/// one writes the user's ISRC into their mood tag. Named on both sides of the diff, a mistake is a
/// compile error.
///
/// The two artist fields are absent on purpose — they are credits and diff structurally, not as
/// the rendered lines this holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TextFields {
    pub title: String,
    pub album: String,
    pub year: String,
    pub original_year: String,
    pub track_number: String,
    pub track_total: String,
    pub disc_number: String,
    pub disc_total: String,
    pub comment: String,
    pub bpm: String,
    pub subtitle: String,
    pub disc_subtitle: String,
    pub grouping: String,
    pub work: String,
    pub movement: String,
    pub movement_number: String,
    pub movement_total: String,
    pub initial_key: String,
    pub mood: String,
    pub language: String,
    pub isrc: String,
    pub copyright: String,
    pub label: String,
    pub catalog_number: String,
    pub barcode: String,
    pub media: String,
    pub release_type: String,
    pub release_country: String,
    pub lyrics: String,
}

/// The whole editable form: the text fields plus the one switch among them.
///
/// The switch rides here rather than as its own argument so "what the user was shown" and "what
/// the user left behind" are one type, compared as a whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FormState {
    pub text: TextFields,
    pub compilation: bool,
}

/// The four multi-value fields, which every format worth the name stores as a list.
///
/// Their own type because they share a contract the text fields don't: each is edited as *rows*
/// and diffed **structurally**, never through the string it renders as. Two credits that render
/// alike are still different credits, and a genre may contain the comma its column joins with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ListFields {
    pub credits: [ArtistCredit; CREDIT_FIELD_COUNT],
    pub genres: GenreList,
    pub roles: RoleCreditEdit,
}

/// The live form, read back off the globals in one place.
///
/// Its counterpart is `populate`'s `field!`, which writes each of these and records the same value
/// as the baseline — so the two sites name the same field and nothing pairs them by position.
pub(super) fn read_form(te: &TagEditor) -> FormState {
    FormState {
        compilation: te.get_compilation(),
        text: read_text_fields(te),
    }
}

fn read_text_fields(te: &TagEditor) -> TextFields {
    TextFields {
        title: te.get_title().to_string(),
        album: te.get_album().to_string(),
        year: te.get_year().to_string(),
        original_year: te.get_original_year().to_string(),
        track_number: te.get_track_number().to_string(),
        track_total: te.get_track_total().to_string(),
        disc_number: te.get_disc_number().to_string(),
        disc_total: te.get_disc_total().to_string(),
        comment: te.get_comment().to_string(),
        bpm: te.get_bpm().to_string(),
        subtitle: te.get_subtitle().to_string(),
        disc_subtitle: te.get_disc_subtitle().to_string(),
        grouping: te.get_grouping().to_string(),
        work: te.get_work().to_string(),
        movement: te.get_movement().to_string(),
        movement_number: te.get_movement_number().to_string(),
        movement_total: te.get_movement_total().to_string(),
        initial_key: te.get_initial_key().to_string(),
        mood: te.get_mood().to_string(),
        language: te.get_language().to_string(),
        isrc: te.get_isrc().to_string(),
        copyright: te.get_copyright().to_string(),
        label: te.get_label().to_string(),
        catalog_number: te.get_catalog_number().to_string(),
        barcode: te.get_barcode().to_string(),
        media: te.get_media().to_string(),
        release_type: te.get_release_type().to_string(),
        release_country: te.get_release_country().to_string(),
        lyrics: te.get_lyrics().to_string(),
    }
}
