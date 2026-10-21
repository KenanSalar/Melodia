//! Whether a row matches a filter box's needle.
//!
//! Every per-view `SearchBar` narrows a list rather than re-querying `tracks_fts`, a second answer
//! to "what does this query match", and this module is what keeps it the same answer for the lists
//! held in memory. What a row is searchable by is `melodia_core`'s `SearchFields`, and the fold is
//! `Needle`'s, covering everything the `unicode61 remove_diacritics 2` tokenizer does. Years are
//! the one deliberate divergence: `Needle::matches_number` is a substring where FTS prefixes.

use melodia_core::entities::track::TrackListRow;
use melodia_core::entities::track_search::SearchFields;
use melodia_core::utils::needle::Needle;

/// Folded substring match across a track row's five text fields and its year. An empty
/// needle matches every row, so an unfiltered list needs no branch.
pub fn track_matches(r: &TrackListRow, needle: &Needle) -> bool {
    SearchFields::from(r).as_array().iter().any(|f| needle.contains(f))
        || needle.matches_number(r.year)
}

#[cfg(test)]
#[path = "tests/row_match_tests.rs"]
mod tests;
