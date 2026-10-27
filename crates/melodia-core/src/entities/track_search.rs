//! What a track is searchable by, and the folded form the store keeps of it.
//!
//! The filter boxes answer one question two ways: the bounded lists walk rows in memory through
//! `Needle::contains`, and the library-sized ones ask the store, which matches the folded
//! `tracks.search_text` column. [`SearchFields`] is what both start from, so the two can only
//! disagree about how a field is folded, never about which fields count.

use crate::entities::track::TrackListRow;
use crate::utils::fold::push_folded;

/// Between two packed fields, here and in the card grids' `search_name`. A filter box is one line
/// and Slint turns a pasted newline into a space, so no match can span two fields; a NUL would be
/// undefined inside `SQLite` TEXT.
pub const FIELD_SEPARATOR: char = '\n';

/// The searchable text of a track, in the order `tracks_fts` lists its columns. `year` joins
/// separately, being an integer.
///
/// `credits` and `file_name` are indexed by FTS and deliberately not carried here: the role
/// credits reach the index through a denormalized column no list renders, and a filename echoes
/// the tags beside it, which an unranked substring filter has no way to de-prioritize as bm25 does.
#[derive(Clone, Copy, Debug)]
pub struct SearchFields<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album_artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
}

impl<'a> SearchFields<'a> {
    pub fn as_array(&self) -> [&'a str; 5] {
        [self.title, self.artist, self.album_artist, self.album, self.genre]
    }

    /// The text `tracks.search_text` holds: every field folded, joined by [`FIELD_SEPARATOR`].
    ///
    /// Never empty, the separators being there whatever the fields hold, which is what lets the
    /// store read an empty column as one not filled yet. **A change to what this returns needs a
    /// migration blanking the column**, or every row filled before it keeps the old format.
    pub fn pack(&self) -> String {
        let fields = self.as_array();
        let text_len: usize = fields.iter().map(|f| f.len()).sum();
        let mut packed = String::with_capacity(text_len + fields.len() - 1);
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                packed.push(FIELD_SEPARATOR);
            }
            push_folded(&mut packed, field);
        }
        packed
    }
}

impl<'a> From<&'a TrackListRow> for SearchFields<'a> {
    fn from(row: &'a TrackListRow) -> Self {
        Self {
            title: &row.title,
            artist: row.artist.as_deref().unwrap_or_default(),
            album_artist: row.album_artist.as_deref().unwrap_or_default(),
            album: row.album.as_deref().unwrap_or_default(),
            genre: row.genre.as_deref().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
#[path = "tests/track_search_tests.rs"]
mod tests;
