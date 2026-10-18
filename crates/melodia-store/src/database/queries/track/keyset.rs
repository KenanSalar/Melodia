//! The ordered ids behind every library-sized track list, filtered and sorted in SQL.
//!
//! A list holding a whole library keeps only these and fetches the rows near its viewport by id
//! (`get_track_list_rows_by_ids`), so its order and its filter are answered here. Each order arm
//! reproduces one arm of `ui::track_sort`, which stays the oracle the two are tested against. Every
//! arm ends in the id, so the order is total and a descending sort is the exact reverse of the
//! ascending one. Rows `ui::track_sort` leaves tied in fetch order come back in id order instead.

use sqlx::{QueryBuilder, Sqlite};

use crate::database::DbPool;
use crate::database::collation::UNICODE_LOWER;
use crate::database::queries::smart_playlist::push_positioned_membership;
use melodia_core::entities::smart_criteria::SmartCriteria;
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
        qb.push(if membership.narrows() { " AND " } else { " WHERE " });
        push_needle(&mut qb, needle);
    }
    qb.push(" ORDER BY ").push(order_by(membership.page(), order, membership.position())).push(")");
    let packed: Option<String> =
        qb.build_query_scalar().persistent(false).fetch_one(db.read()).await?;
    packed.map_or_else(|| Ok(Vec::new()), |packed| unpack_ids(&packed))
}

/// The ids `group_concat` joined, in order. The aggregate is NULL where nothing matched, which
/// [`track_ids`] answers before getting here.
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

/// Pushes the condition `needle` filters by, for a caller that skips an empty needle.
///
/// The folded column, plus the year in decimal wherever the needle can name one, which is
/// `Needle::matches_number`'s substring rule. A row the backfill hasn't reached yet matches every
/// needle rather than none, so the window after an upgrade can only widen a list.
pub(super) fn push_needle(qb: &mut QueryBuilder<Sqlite>, needle: &Needle) {
    qb.push("(search_text = '' OR instr(search_text, ")
        .push_bind(needle.as_str().to_owned())
        .push(") > 0");
    if needle.is_number() {
        qb.push(" OR instr(CAST(year AS TEXT), ")
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
    let dir = match order.dir {
        SortDir::Asc => "ASC",
        SortDir::Desc => "DESC",
    };
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

/// A nullable text column read the way `ui::track_sort` reads it: missing as `""`, which a bare
/// NULL would sort ahead of, then folded through `to_lowercase()`.
fn lowered(column: &str) -> String {
    format!("COALESCE({column}, '') COLLATE {UNICODE_LOWER}")
}

/// Where a list's tracks come from.
enum Membership {
    Library,
    Favorites,
    Album(i64),
    Artist(i64),
    Genre(i64),
    ManualPlaylist(i64),
    SmartPlaylist(SmartCriteria),
}

impl Membership {
    /// A playlist is the one scope that can't say where its tracks come from without asking: a
    /// smart one has no `playlist_items` rows and resolves through its stored criteria. One that
    /// no longer exists reads as an empty manual playlist.
    async fn resolve(db: &DbPool, scope: TrackListScope) -> Result<Self, AppError> {
        Ok(match scope {
            TrackListScope::Library => Self::Library,
            TrackListScope::Favorites => Self::Favorites,
            TrackListScope::Album(id) => Self::Album(id),
            TrackListScope::Artist(id) => Self::Artist(id),
            TrackListScope::Genre(id) => Self::Genre(id),
            TrackListScope::Playlist(id) => {
                let verdict: Option<(bool, Option<String>)> =
                    sqlx::query_as("SELECT is_smart, smart_criteria FROM playlists WHERE id = ?")
                        .bind(id)
                        .fetch_optional(db.read())
                        .await?;
                match verdict {
                    Some((true, criteria)) => {
                        Self::SmartPlaylist(SmartCriteria::from_json_opt(criteria.as_deref()))
                    }
                    _ => Self::ManualPlaylist(id),
                }
            }
        })
    }

    fn page(&self) -> Page {
        match self {
            Self::Library | Self::Favorites => Page::Songs,
            _ => Page::Detail,
        }
    }

    /// Whether [`Self::push_source`] already opened a `WHERE`.
    fn narrows(&self) -> bool {
        !matches!(self, Self::Library | Self::SmartPlaylist(_))
    }

    fn position(&self) -> Option<&'static str> {
        match self {
            Self::ManualPlaylist(_) | Self::SmartPlaylist(_) => Some("m.position"),
            _ => None,
        }
    }

    /// Pushes the `FROM`, and the `WHERE` that narrows it to this membership where one does. An
    /// artist or genre is matched through its join table with `IN` rather than a join, for
    /// `get_tracks_by_artist_for_list`'s reason: a credit naming one artist twice would join a
    /// track in twice.
    fn push_source(&self, qb: &mut QueryBuilder<Sqlite>) {
        match self {
            Self::Library => {
                qb.push("tracks t");
            }
            Self::Favorites => {
                qb.push("tracks t WHERE t.is_favorite = TRUE");
            }
            Self::Album(id) => {
                qb.push("tracks t WHERE t.album_id = ").push_bind(*id);
            }
            Self::Artist(id) => {
                qb.push(
                    "tracks t WHERE t.id IN (SELECT track_id FROM track_artists WHERE artist_id = ",
                )
                .push_bind(*id)
                .push(")");
            }
            Self::Genre(id) => {
                qb.push(
                    "tracks t WHERE t.id IN (SELECT track_id FROM track_genres WHERE genre_id = ",
                )
                .push_bind(*id)
                .push(")");
            }
            Self::ManualPlaylist(id) => {
                qb.push(
                    "playlist_items m JOIN tracks t ON t.id = m.track_id WHERE m.playlist_id = ",
                )
                .push_bind(*id);
            }
            Self::SmartPlaylist(criteria) => {
                qb.push("(");
                push_positioned_membership(qb, criteria);
                qb.push(") m JOIN tracks t ON t.id = m.id");
            }
        }
    }
}
