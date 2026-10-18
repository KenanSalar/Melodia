//! Whether a row matches a filter box's needle.
//!
//! Every per-view `SearchBar` narrows a list rather than re-querying `tracks_fts`, a second answer
//! to "what does this query match", and this module is what keeps it the same answer for the lists
//! held in memory. What a row is searchable by is `melodia_core`'s `SearchFields`, and the fold is
//! `Needle`'s, covering everything the `unicode61 remove_diacritics 2` tokenizer does. Years are
//! the one deliberate divergence: `Needle::matches_number` is a substring where FTS prefixes.

use melodia_core::entities::track::{MostPlayedFavorite, TrackListRow};
use melodia_core::entities::track_search::SearchFields;
use melodia_core::utils::needle::Needle;

/// Any field, or the year's decimal form, containing `needle`.
fn any_field_matches(fields: SearchFields<'_>, year: Option<i32>, needle: &Needle) -> bool {
    fields.as_array().iter().any(|f| needle.contains(f)) || needle.matches_number(year)
}

/// Folded substring match across a track row's five text fields and its year. An empty
/// needle matches every row, so an unfiltered list needs no branch.
pub fn track_matches(r: &TrackListRow, needle: &Needle) -> bool {
    any_field_matches(SearchFields::from(r), r.year, needle)
}

/// [`track_matches`] over a Most Played card, which carries the same five fields for the reason
/// `SearchFields`' conversion from it gives.
pub fn most_played_matches(t: &MostPlayedFavorite, needle: &Needle) -> bool {
    any_field_matches(SearchFields::from(t), t.year, needle)
}

#[cfg(test)]
#[path = "tests/row_match_tests.rs"]
mod tests;
