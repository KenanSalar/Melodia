//! The ordered ids behind every library-sized track list, filtered and sorted in SQL.
//!
//! A list holding a whole library keeps only these and fetches the rows near its viewport by id
//! (`get_track_list_rows_by_ids`), so its order and its filter are answered here. Each order arm
//! reproduces one arm of `ui::track_sort`, which stays the oracle the two are tested against. Every
//! arm ends in the id, so the order is total and a descending sort is the exact reverse of the
//! ascending one. Rows `ui::track_sort` leaves tied in fetch order come back in id order instead.

use super::membership::Membership;
use crate::database::DbPool;
use crate::database::collation::UNICODE_LOWER;
use crate::database::sql::Sql;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder, TrackSortField};
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;

/// Where an untagged track number sorts, mirroring `ui::track_sort`'s `i32::MAX`: after every
/// tagged one, where `SQLite` would put a NULL first.
const UNTAGGED_TRACK_LAST: i32 = i32::MAX;

/// The longest a Songs row's length reads, the most milliseconds the `i32` it displays holds.
/// `idx_tracks_length_order` carries it written out, so another cap still sorts right and walks
/// nothing.
const SONGS_LENGTH_CAP_MS: i32 = i32::MAX;

/// A Songs row's year, a missing one as `0`. `idx_tracks_year_order` carries this spelling, the
/// only one an index on an expression serves.
const SONGS_YEAR: &str = "COALESCE(t.year, 0)";

/// The scan's natural-order key, folded the way `to_ascii_lowercase` folds it.
const NATURAL: &str = "COALESCE(t.sort_key, '') COLLATE NOCASE";

/// [`NATURAL`] named bare, so a walked list reads `idx_tracks_sort_key` in order. The same order,
/// every track write filling `sort_key`.
const NATURAL_WALKED: &str = "t.sort_key COLLATE NOCASE";

/// The ids of every track in `scope` that `needle` matches, in `order`.
pub async fn track_ids(
    db: &DbPool,
    scope: TrackListScope,
    order: TrackOrder,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let needle = needle.clone();
    db.read(move |conn| {
        let membership = Membership::resolve(conn, scope)?;
        let mut sql = Sql::new("SELECT t.id FROM ");
        membership.push_source(&mut sql);
        if !needle.is_empty() {
            membership.push_and(&mut sql);
            push_needle(&mut sql, &needle);
        }
        sql.push(" ORDER BY ").push_vouched(&order_by(
            Page::of(&membership, &needle),
            order,
            membership.position(),
        ));
        sql.fetch_all(conn)
    })
    .await
}

/// The order a detail page defaults to for `field`, ascending, for a query over `tracks t` that
/// has to agree with that page without being one of its keysets.
pub(crate) fn detail_order(field: TrackSortField) -> String {
    order_by(Page::Detail, TrackOrder { field, dir: SortDir::Asc }, None)
}

/// Pushes the condition `needle` filters a track list by, for a caller that skips an empty needle.
pub(super) fn push_needle(sql: &mut Sql, needle: &Needle) {
    push_folded_match(sql, needle, "search_text", Some("year"));
}

/// Pushes `needle` against the folded `column`, plus `year` in decimal wherever the needle can
/// name one, which is `Needle::matches_number`'s substring rule.
///
/// A row the backfill hasn't reached yet matches every needle rather than none, so the window
/// after an upgrade can only widen a list.
pub(crate) fn push_folded_match(
    sql: &mut Sql,
    needle: &Needle,
    column: &'static str,
    year: Option<&'static str>,
) {
    sql.push("(")
        .push(column)
        .push(" = '' OR instr(")
        .push(column)
        .push(", ")
        .push_bind(needle.as_str())
        .push(") > 0");
    if let Some(year) = year
        && needle.is_number()
    {
        sql.push(" OR instr(CAST(")
            .push(year)
            .push(" AS TEXT), ")
            .push_bind(needle.as_str())
            .push(") > 0");
    }
    sql.push(")");
}

/// Which page a list sits on, which decides its tie-break and how a row reads.
#[derive(Clone, Copy)]
enum Page {
    /// My Library's and Favorites' Songs tabs. A missing year sorts as `0` beside an explicit one
    /// rather than ahead of every year, and the length is clamped to what an `i32` of milliseconds
    /// holds, the values a Songs row displays.
    Songs(IndexUse),
    /// The album, artist, genre and playlist pages, whose rows read as stored, ties going to the
    /// title.
    Detail,
}

/// How an order names a term an index carries in that order.
#[derive(Clone, Copy)]
enum IndexUse {
    /// Named bare, so the whole library, unfiltered, comes off the term's index already in order.
    Walked,
    /// Behind `+` or `COALESCE`, so `SQLite` finds a filter's or a scope's rows first and sorts
    /// those, where walking the index would look up every row the filter then drops.
    Sorted,
}

impl Page {
    fn of(membership: &Membership, needle: &Needle) -> Self {
        match membership {
            Membership::Library if needle.is_empty() => Self::Songs(IndexUse::Walked),
            Membership::Library | Membership::Favorites => Self::Songs(IndexUse::Sorted),
            _ => Self::Detail,
        }
    }

    fn natural(self) -> &'static str {
        match self {
            Self::Songs(IndexUse::Walked) => NATURAL_WALKED,
            Self::Songs(IndexUse::Sorted) | Self::Detail => NATURAL,
        }
    }

    fn indexed(self, term: &str) -> String {
        match self {
            Self::Songs(IndexUse::Walked) => term.to_owned(),
            Self::Songs(IndexUse::Sorted) | Self::Detail => format!("+{term}"),
        }
    }

    /// The stored order key `column` names (`queries::track::order_keys`), `ui::track_sort`'s
    /// lowercase of its field.
    fn key(self, column: &str) -> String {
        self.indexed(&format!("t.{column}"))
    }

    fn tie_break(self) -> String {
        match self {
            Self::Songs(_) => self.natural().to_owned(),
            Self::Detail => format!("t.title COLLATE {UNICODE_LOWER}"),
        }
    }

    fn year(self) -> String {
        match self {
            Self::Songs(_) => self.indexed(SONGS_YEAR),
            Self::Detail => "t.year".to_owned(),
        }
    }

    fn length(self) -> String {
        match self {
            Self::Songs(_) => {
                self.indexed(&format!("MIN(MAX(t.duration_ms, 0), {SONGS_LENGTH_CAP_MS})"))
            }
            Self::Detail => "t.duration_ms".to_owned(),
        }
    }
}

/// The `ORDER BY` body for `order` on `page`. `position` is the list's own order where it has
/// one, a playlist's.
fn order_by(page: Page, order: TrackOrder, position: Option<&str>) -> String {
    let dir = sql_direction(order.dir);
    let tie = page.tie_break();
    let then_tie = |key: &str| format!("{key} {dir}, {tie} {dir}, t.id {dir}");
    match (order.field, position) {
        // Only the track term flips: disc and the tie-break stay ascending.
        (TrackSortField::TrackNumber, _) => format!(
            "COALESCE(NULLIF(t.disc_number, 0), 1) ASC, \
             COALESCE(NULLIF(t.track_number, 0), {UNTAGGED_TRACK_LAST}) {dir}, \
             {tie} ASC, t.id ASC"
        ),
        (TrackSortField::Artist, _) => then_tie(&page.key("artist_key")),
        (TrackSortField::Album, _) => then_tie(&page.key("album_key")),
        (TrackSortField::Genre, _) => then_tie(&page.key("genre_key")),
        (TrackSortField::Year, _) => then_tie(&page.year()),
        (TrackSortField::Length, _) => then_tie(&page.length()),
        (TrackSortField::Position, Some(position)) => format!("{position} {dir}, t.id {dir}"),
        // On the Songs tabs the natural key is the tie-break too, so it is compared once.
        (TrackSortField::Title | TrackSortField::Position, _) => match page {
            Page::Songs(_) => format!("{} {dir}, t.id {dir}", page.natural()),
            Page::Detail => then_tie(NATURAL),
        },
    }
}

pub(crate) fn sql_direction(dir: SortDir) -> &'static str {
    match dir {
        SortDir::Asc => "ASC",
        SortDir::Desc => "DESC",
    }
}

#[cfg(test)]
#[path = "../tests/keyset_tests.rs"]
mod tests;
