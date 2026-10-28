//! The in-memory sort for track-list rows already fetched: every list held in memory answers a
//! header click through [`sort_track_rows_by`], and the SQL keyset ordering the two Songs tabs
//! and the four details reproduces its arms one for one, `crates/melodia/tests/cross_tier.rs`
//! holding the two together.
//!
//! `sort_by_cached_key` because a plain `sort_by` would re-fold both operands on every
//! comparison. Branching once on `field` picks the key type per arm, which is what lets the
//! natural-order arm drop its tie-breaker. The comparator reads a trait rather than a row type,
//! so the parity tests can hand it a row read the way the Songs tabs display it.

use melodia_core::entities::track::TrackListRow as RsTrackListRow;

/// The fields a track-list sort compares. The flattening rules live here rather than in the
/// arms, so every implementor is held to the same ones.
pub trait TrackSortFields {
    /// Effective disc number; `NULL` and `0` both mean disc 1.
    fn disc(&self) -> i32;
    /// Effective track number, widened so the sentinel can be negated for a
    /// descending sort. `NULL` and `0` sort last ascending.
    fn track(&self) -> i64;
    fn artist(&self) -> &str;
    fn album(&self) -> &str;
    fn genre(&self) -> &str;
    /// `None` sorts first ascending (NULLs-first), matching `SQLite`.
    fn year(&self) -> Option<i32>;
    fn duration_ms(&self) -> i64;
    /// Natural-order key, raw — the comparator owns the case fold.
    fn sort_key(&self) -> &str;
}

impl TrackSortFields for RsTrackListRow {
    fn disc(&self) -> i32 {
        match self.disc_number {
            Some(d) if d != 0 => d,
            _ => 1,
        }
    }

    fn track(&self) -> i64 {
        match self.track_number {
            Some(n) if n != 0 => i64::from(n),
            _ => i64::from(i32::MAX),
        }
    }

    fn artist(&self) -> &str {
        self.artist.as_deref().unwrap_or("")
    }

    fn album(&self) -> &str {
        self.album.as_deref().unwrap_or("")
    }

    fn genre(&self) -> &str {
        self.genre.as_deref().unwrap_or("")
    }

    fn year(&self) -> Option<i32> {
        self.year
    }

    fn duration_ms(&self) -> i64 {
        self.duration_ms
    }

    fn sort_key(&self) -> &str {
        self.sort_key.as_deref().unwrap_or("")
    }
}

/// Sort `items` in place by `field` / `dir`, `row` projecting each element to its
/// [`TrackSortFields`] and `secondary` yielding the lowercased tie-breaker (the title on a
/// detail, the file name on Browse).
///
/// Unknown and `"title"` fields fall back to the natural-order `sort_key`. `"desc"` reverses the
/// whole order for every field but `track_number`, where only the track component flips, by
/// negating its sentinel, and disc and secondary stay ascending.
pub fn sort_track_rows_by<T, F, R, S>(items: &mut [T], field: &str, dir: &str, row: R, secondary: S)
where
    F: TrackSortFields + ?Sized,
    R: Fn(&T) -> &F,
    S: Fn(&T) -> String,
{
    sort_rows(items, field, dir, row, secondary, false);
}

/// [`sort_track_rows_by`] with `secondary_is_natural_key`, for [`compute_track_order`] alone,
/// whose tie-breaker is the value the natural-order arm already compares on: `(k, k)` and `k`
/// are the same comparison, and the second copy cost a `String` per row on the default sort.
fn sort_rows<T, F, R, S>(
    items: &mut [T],
    field: &str,
    dir: &str,
    row: R,
    secondary: S,
    secondary_is_natural_key: bool,
) where
    F: TrackSortFields + ?Sized,
    R: Fn(&T) -> &F,
    S: Fn(&T) -> String,
{
    let desc = dir == "desc";

    // Never `.reverse()` here, which would flip disc and secondary too.
    if field == "track_number" {
        items.sort_by_cached_key(|t| {
            let r = row(t);
            let track = r.track();
            (r.disc(), if desc { -track } else { track }, secondary(t))
        });
        return;
    }

    match field {
        "artist" => {
            items.sort_by_cached_key(|t| (row(t).artist().to_lowercase(), secondary(t)));
        }
        "album" => {
            items.sort_by_cached_key(|t| (row(t).album().to_lowercase(), secondary(t)));
        }
        "genre" => {
            items.sort_by_cached_key(|t| (row(t).genre().to_lowercase(), secondary(t)));
        }
        "year" => {
            items.sort_by_cached_key(|t| (row(t).year(), secondary(t)));
        }
        "length" => {
            items.sort_by_cached_key(|t| (row(t).duration_ms(), secondary(t)));
        }
        _ if secondary_is_natural_key => {
            items.sort_by_cached_key(|t| row(t).sort_key().to_ascii_lowercase());
        }
        _ => items.sort_by_cached_key(|t| (row(t).sort_key().to_ascii_lowercase(), secondary(t))),
    }
    if desc {
        items.reverse();
    }
}

/// Sort the way a detail page orders, the title breaking ties: the comparator `cross_tier` holds
/// the details' keyset arms to.
pub fn sort_track_list_rows(rows: &mut [RsTrackListRow], field: &str, dir: &str) {
    sort_track_rows_by(rows, field, dir, |r| r, |r| r.title.to_lowercase());
}

/// The Songs tabs' display order of `rows`, as a permutation, ties broken on the natural
/// `sort_key` the Songs keyset is held against.
pub fn compute_track_order<F: TrackSortFields>(rows: &[F], field: &str, dir: &str) -> Vec<usize> {
    let mut indexed: Vec<(usize, &F)> = rows.iter().enumerate().collect();
    sort_rows(&mut indexed, field, dir, |t| t.1, |t| t.1.sort_key().to_ascii_lowercase(), true);
    indexed.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
#[path = "tests/track_sort_tests.rs"]
mod tests;
