//! The pure folds a hero band's chips are built from, apart from [`crate::ui::hero_chips`] so
//! they test without an `AppWindow`.
//!
//! They run on the worker that fetched their input, never inside an `upgrade_in_event_loop`, and
//! hand back narrow `Copy` results. A detail's spread is SQL's to count (`ScopeFacts`).

use std::collections::HashSet;

use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::track::TrackListRow;
use melodia_core::entities::track_list::ScopeFacts;

#[cfg(test)]
#[path = "tests/hero_folds_tests.rs"]
mod tests;

/// How many distinct artists and albums a track list spans, which a genre's or playlist's stats
/// row can't say.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct HeroFold {
    pub artists: i32,
    pub albums: i32,
}

impl From<&ScopeFacts> for HeroFold {
    fn from(facts: &ScopeFacts) -> Self {
        Self { artists: clamp_i64_to_i32(facts.artists), albums: clamp_i64_to_i32(facts.albums) }
    }
}

/// Counts a held track list's spread, which `ScopeFacts` answers in SQL for a list of ids. A track
/// with no album is in none, rather than pooled into what would read as one more album.
pub fn fold_tracks(rows: &[TrackListRow]) -> HeroFold {
    let mut artists: HashSet<i64> = HashSet::new();
    let mut albums: HashSet<i64> = HashSet::new();
    for row in rows {
        if let Some(id) = row.artist_id {
            artists.insert(id);
        }
        if let Some(id) = row.album_id {
            albums.insert(id);
        }
    }
    HeroFold { artists: len_as_i32(artists.len()), albums: len_as_i32(albums.len()) }
}

/// The genre most of a set of tracks carry, or `None` without a majority, so a compilation that
/// spans several gets no chip. `lines` is `queries::track::genre_lines`.
///
/// Tallied per genre, not per rendered line: tracks all `Rock` and half also `Metal` are two lines
/// and no majority, where every track shares `Rock`. Each track counts once per name, and where
/// several names clear the majority the first in tag order wins.
pub fn dominant_genre(lines: &[(String, i64)]) -> Option<String> {
    /// Share of the tracks the winner has to hold to be worth stating.
    const MAJORITY: i64 = 2;

    let mut tally: Vec<(&str, i64)> = Vec::new();
    // Reused across lines, so the whole fold allocates once however many there are.
    let mut in_line: Vec<&str> = Vec::new();
    let mut tagged = 0i64;
    for (line, tracks) in lines {
        // A line can spell one name twice: `names_in_line` splits a rendered column, and a genre
        // whose own name holds the separator comes back as two. The denominator is tracks.
        in_line.clear();
        for name in GenreList::names_in_line(line) {
            if !in_line.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
                in_line.push(name);
            }
        }
        if in_line.is_empty() {
            continue;
        }
        tagged += tracks;
        for &name in &in_line {
            match tally.iter_mut().find(|(seen, _)| seen.eq_ignore_ascii_case(name)) {
                Some((_, count)) => *count += tracks,
                None => tally.push((name, *tracks)),
            }
        }
    }
    // Strict `>` rather than `max_by_key`, which keeps the *last* of equal maxima: where two names
    // both clear the majority, the first is the one tag order calls primary.
    let (name, count) =
        tally.into_iter().reduce(|best, next| if next.1 > best.1 { next } else { best })?;
    (count * MAJORITY > tagged).then(|| name.to_owned())
}

/// The release-year span of an artist's albums; a single year answers `(y, y)`.
pub fn year_span(albums: &[AlbumStats]) -> Option<(i32, i32)> {
    let mut years = albums.iter().filter_map(|a| a.year).filter(|y| *y > 0);
    let first = years.next()?;
    Some(years.fold((first, first), |(lo, hi), y| (lo.min(y), hi.max(y))))
}
