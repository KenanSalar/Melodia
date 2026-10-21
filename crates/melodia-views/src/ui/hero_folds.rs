//! The pure folds a hero band's chips are built from.
//!
//! Split out of [`crate::ui::hero_chips`], which is the *channel*. Nothing here touches
//! Slint or that record: each takes what a query answered and returns a `Copy` or a
//! `String`, so the two halves share no state and the folds are testable without an
//! `AppWindow`.
//!
//! **These run on the worker that fetched their input, never inside an
//! `upgrade_in_event_loop`**, which is why the results are narrow and ride into the
//! closure as finished values. A detail's spread is SQL's to count (`ScopeFacts`), its
//! page holding no rows to fold.

use std::collections::HashSet;

use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::track::TrackListRow;
use melodia_core::entities::track_list::ScopeFacts;

#[cfg(test)]
#[path = "tests/hero_folds_tests.rs"]
mod tests;

/// How many distinct artists and albums a track list spans — the two facts a list-shaped
/// hero can state that its stats row can't, a genre or playlist knowing how many
/// *tracks* it holds and nothing about their spread. Two words wide, so it crosses an
/// `upgrade_in_event_loop` for free.
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

/// Count the distinct artists and albums a held track list spans, the fold `ScopeFacts` answers in
/// SQL for a list that holds only ids. Keyed on ids rather than names: an `i64` hashes far cheaper,
/// and a track with no album belongs to none — so `None` is skipped rather than pooled into an
/// "unknown" bucket that would read as one more album.
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

/// The genre most of a set of tracks is tagged with, or `None` when it is split evenly
/// enough that naming one would misrepresent the rest. An album is usually
/// single-genre, so this reads as "the album's genre" there; a compilation that
/// genuinely spans several gets no chip rather than whichever won by a track.
///
/// `lines` is each rendered genre line with how many tracks carry it, commonest first, which is
/// `queries::track::genre_lines`.
///
/// **Tallied per genre, not per rendered line.** A row carries every genre it is tagged with in
/// one string, so an album whose tracks are all `Rock` and half also `Metal` holds two distinct
/// lines and no majority, where what every track shares is `Rock`. The denominator stays *tracks*,
/// each contributing at most once to any one name, so the winner is a genre more than half the
/// tagged tracks carry. Several can be, and the first in the commonest line's tag order takes it.
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

/// The span of release years across an artist's albums, or `None` when no album
/// carries one. A single year answers `(y, y)` and is rendered without a dash.
pub fn year_span(albums: &[AlbumStats]) -> Option<(i32, i32)> {
    let mut years = albums.iter().filter_map(|a| a.year).filter(|y| *y > 0);
    let first = years.next()?;
    Some(years.fold((first, first), |(lo, hi), y| (lo.min(y), hi.max(y))))
}
