//! The pure folds a hero band's chips are built from.
//!
//! Split out of [`crate::ui::hero_chips`], which is the *channel* — the
//! `thread_local` record of what is on screen, the builders behind
//! `ChipLabels`, and the six publishers. Nothing here touches Slint or that
//! record: each of these takes a slice and returns a `Copy` (or a `String`), so
//! the two halves share no state and the folds are testable without an
//! `AppWindow`. Two of them, [`dominant_genre`] and [`year_span`], were never
//! called from `hero_chips` at all — only from `albums/detail.rs` and
//! `artists/detail.rs`.
//!
//! **These run on the worker that fetched the rows, never inside an
//! `upgrade_in_event_loop`.** A broad genre's track list is the longest in the
//! app and has no business being hashed on the UI thread; that is the whole
//! reason the results are `Copy` and narrow — they ride into the closure as
//! finished values rather than being derived there. `hero_chips`' own module
//! doc argues the other half, that a publisher folds nothing.

use std::collections::HashSet;

use crate::entities::album::AlbumStats;
use crate::entities::track::{MostPlayedFavorite, TrackListRow};
use crate::ui::util::len_as_i32;

#[cfg(test)]
#[path = "tests/hero_folds_tests.rs"]
mod tests;

/// How many distinct artists and albums a track list spans.
///
/// The two facts a list-shaped hero can state that its stats row can't: a genre
/// or a playlist knows how many *tracks* it holds and nothing about their
/// spread. `Copy` and two words wide, so it crosses an `upgrade_in_event_loop`
/// for free.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct HeroFold {
    pub artists: i32,
    pub albums: i32,
}

/// What the Most Played tab sums to.
///
/// Its own totals, never the Songs tab's: the query behind it is
/// `is_favorite = TRUE AND play_count > 0`, a strict subset, so borrowing the
/// Songs duration would overstate it by every favourite never played.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct MostPlayedTotals {
    pub tracks: i32,
    pub duration_ms: i64,
    pub plays: i32,
}

/// Count the distinct artists and albums a track list spans.
///
/// Keyed on the ids rather than the names: an `i64` hashes far cheaper than a
/// `String` over a list this size, and a track with no album genuinely belongs
/// to none, so `None` is skipped rather than pooled into an "unknown" bucket
/// that would read as one more album.
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
    HeroFold {
        artists: len_as_i32(artists.len()),
        albums: len_as_i32(albums.len()),
    }
}

/// Sum the Most Played tab's own totals off its cached rows.
///
/// One walk rather than two: Recently Played's copy of this list is uncapped and
/// library-wide, and a row is mostly `String`, so a `sum` per field is a second
/// pass over more memory than fits a cache to reach two `Copy` fields inside it.
pub fn fold_most_played(rows: &[MostPlayedFavorite]) -> MostPlayedTotals {
    let (duration_ms, plays) = rows
        .iter()
        .fold((0i64, 0i32), |(duration_ms, plays), row| {
            (duration_ms + row.duration_ms, plays + row.play_count)
        });
    MostPlayedTotals {
        tracks: len_as_i32(rows.len()),
        duration_ms,
        plays,
    }
}

/// The genre most of a track list is tagged with, or `None` when it is split
/// evenly enough that naming one would misrepresent the rest.
///
/// An album is usually single-genre, so this reads as "the album's genre"
/// there; a compilation that genuinely spans several gets no chip rather than
/// whichever one happened to win by a track.
pub fn dominant_genre(rows: &[TrackListRow]) -> Option<String> {
    /// Share of the tracks the winner has to hold to be worth stating.
    const MAJORITY: usize = 2;

    let mut tally: Vec<(&str, usize)> = Vec::new();
    let mut tagged = 0usize;
    for genre in rows.iter().filter_map(|r| r.genre.as_deref()) {
        if genre.is_empty() {
            continue;
        }
        tagged += 1;
        match tally.iter_mut().find(|(name, _)| *name == genre) {
            Some((_, count)) => *count += 1,
            None => tally.push((genre, 1)),
        }
    }
    let (name, count) = tally.into_iter().max_by_key(|&(_, count)| count)?;
    (count * MAJORITY > tagged).then(|| name.to_owned())
}

/// The span of release years across an artist's albums, or `None` when no album
/// carries one. A single year answers `(y, y)` and is rendered without a dash.
pub fn year_span(albums: &[AlbumStats]) -> Option<(i32, i32)> {
    let mut years = albums.iter().filter_map(|a| a.year).filter(|y| *y > 0);
    let first = years.next()?;
    Some(years.fold((first, first), |(lo, hi), y| (lo.min(y), hi.max(y))))
}
