//! The ordered ids behind every library-sized track list, filtered and sorted in SQL.
//!
//! A list holding a whole library keeps only these and fetches the rows near its viewport by id
//! (`get_track_list_rows_by_ids`), so its order and its filter are answered here. Each order arm
//! reproduces one arm of `ui::track_sort`, which stays the oracle the two are tested against. Every
//! arm ends in the id, so the order is total and a descending sort is the exact reverse of the
//! ascending one. Rows `ui::track_sort` leaves tied in fetch order come back in id order instead.

use sqlx::{QueryBuilder, Sqlite};

use super::membership::Membership;
use crate::database::DbPool;
use crate::database::collation::UNICODE_LOWER;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder, TrackSortField};
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;

/// Where an untagged track number sorts, mirroring `ui::track_sort`'s `i32::MAX`: after every
/// tagged one, where `SQLite` would put a NULL first.
const UNTAGGED_TRACK_LAST: i32 = i32::MAX;

/// The longest a Songs row's length reads, the most milliseconds the `i32` it displays holds.
const SONGS_LENGTH_CAP_MS: i32 = i32::MAX;

/// The scan's natural-order key, folded the way `to_ascii_lowercase` folds it.
const NATURAL: &str = "COALESCE(t.sort_key, '') COLLATE NOCASE";

/// The ids of every track in `scope` that `needle` matches, in `order`.
///
/// They come back as one comma-joined row rather than a row each: sqlx hands every row across
/// from its connection's worker thread on its own, which over a library costs more than the sort.
///
/// The order is the subquery's, not the aggregate's own `ORDER BY`, which sorts slower wherever an
/// index can't answer it. `SQLite`'s flattener keeps a FROM-clause subquery's `ORDER BY` under
/// any aggregate but `count`, `min` and `max`, and `cross_tier` holds every arm to it.
pub async fn track_ids(
    db: &DbPool,
    scope: TrackListScope,
    order: TrackOrder,
    needle: &Needle,
) -> Result<Vec<i64>, AppError> {
    let membership = Membership::resolve(db, scope).await?;
    let mut qb =
        QueryBuilder::<Sqlite>::new("SELECT group_concat(id, ',') FROM (SELECT t.id FROM ");
    membership.push_source(&mut qb);
    if !needle.is_empty() {
        membership.push_and(&mut qb);
        push_needle(&mut qb, needle);
    }
    qb.push(" ORDER BY ")
        .push(order_by(Page::of(&membership), order, membership.position()))
        .push(")");
    fetch_packed_ids(db, qb).await
}

/// The ids `qb` reads back as one `group_concat(id, ',')`, in the order joined. The aggregate is
/// NULL where nothing matched.
pub(crate) async fn fetch_packed_ids(
    db: &DbPool,
    mut qb: QueryBuilder<Sqlite>,
) -> Result<Vec<i64>, AppError> {
    let packed: Option<String> =
        qb.build_query_scalar().persistent(false).fetch_one(db.read()).await?;
    packed.map_or_else(|| Ok(Vec::new()), |packed| unpack_ids(&packed))
}

fn unpack_ids(packed: &str) -> Result<Vec<i64>, AppError> {
    let mut ids = Vec::with_capacity(packed.bytes().filter(|&b| b == b',').count() + 1);
    for id in packed.split(',') {
        ids.push(id.parse().map_err(|e| sqlx::Error::Decode(Box::new(e)))?);
    }
    Ok(ids)
}

/// The order a detail page defaults to for `field`, ascending, for a query over `tracks t` that
/// has to agree with that page without being one of its keysets.
pub(super) fn detail_order(field: TrackSortField) -> String {
    order_by(Page::Detail, TrackOrder { field, dir: SortDir::Asc }, None)
}

/// Pushes the condition `needle` filters a track list by, for a caller that skips an empty needle.
pub(super) fn push_needle(qb: &mut QueryBuilder<Sqlite>, needle: &Needle) {
    push_folded_match(qb, needle, "search_text", Some("year"));
}

/// Pushes `needle` against the folded `column`, plus `year` in decimal wherever the needle can
/// name one, which is `Needle::matches_number`'s substring rule.
///
/// A row the backfill hasn't reached yet matches every needle rather than none, so the window
/// after an upgrade can only widen a list.
pub(crate) fn push_folded_match(
    qb: &mut QueryBuilder<Sqlite>,
    needle: &Needle,
    column: &'static str,
    year: Option<&'static str>,
) {
    qb.push(format!("({column} = '' OR instr({column}, "))
        .push_bind(needle.as_str().to_owned())
        .push(") > 0");
    if let Some(year) = year
        && needle.is_number()
    {
        qb.push(format!(" OR instr(CAST({year} AS TEXT), "))
            .push_bind(needle.as_str().to_owned())
            .push(") > 0");
    }
    qb.push(")");
}

/// Which page a list sits on, which decides its tie-break and how a row reads.
#[derive(Clone, Copy)]
enum Page {
    /// My Library's and Favorites' Songs tabs. A missing year sorts as `0` beside an explicit one
    /// rather than ahead of every year, and the length is clamped to what an `i32` of milliseconds
    /// holds, the values a Songs row displays.
    Songs,
    /// The album, artist, genre and playlist pages, whose rows read as stored, ties going to the
    /// title.
    Detail,
}

impl Page {
    fn of(membership: &Membership) -> Self {
        match membership {
            Membership::Library | Membership::Favorites => Self::Songs,
            _ => Self::Detail,
        }
    }

    fn tie_break(self) -> String {
        match self {
            Self::Songs => NATURAL.to_owned(),
            Self::Detail => format!("t.title COLLATE {UNICODE_LOWER}"),
        }
    }

    fn year(self) -> &'static str {
        match self {
            Self::Songs => "COALESCE(t.year, 0)",
            Self::Detail => "t.year",
        }
    }

    fn length(self) -> String {
        match self {
            Self::Songs => format!("MIN(MAX(t.duration_ms, 0), {SONGS_LENGTH_CAP_MS})"),
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
        (TrackSortField::Artist, _) => then_tie(&lowered("t.artist")),
        (TrackSortField::Album, _) => then_tie(&lowered("t.album")),
        (TrackSortField::Genre, _) => then_tie(&lowered("t.genre")),
        (TrackSortField::Year, _) => then_tie(page.year()),
        (TrackSortField::Length, _) => then_tie(&page.length()),
        (TrackSortField::Position, Some(position)) => format!("{position} {dir}, t.id {dir}"),
        // On the Songs tabs the natural key is the tie-break too, so it is compared once.
        (TrackSortField::Title | TrackSortField::Position, _) => match page {
            Page::Songs => format!("{NATURAL} {dir}, t.id {dir}"),
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

/// A nullable text column read the way `ui::track_sort` reads it: missing as `""`, which a bare
/// NULL would sort ahead of, then folded through `to_lowercase()`.
fn lowered(column: &str) -> String {
    format!("COALESCE({column}, '') COLLATE {UNICODE_LOWER}")
}
