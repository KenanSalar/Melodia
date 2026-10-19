//! The folded names the card grids filter by: what each kind packs into `search_name`, the
//! backfill that fills the rows written before the column existed, and the one-row refill the
//! writers call where the name they hold isn't the whole of it.
//!
//! Each kind's inputs are read by one SELECT, the backfill's and the refill's both, so an album's
//! artist line is the one its card shows (`album_stats` reads `COALESCE(artist_credit, name)`),
//! the unknown artist's seeded name included, whichever path wrote it.

use sqlx::{AssertSqlSafe, SqliteConnection};

use crate::database::{
    CASE_BINDS_PER_ROW, DbPool, MAX_BINDS_PER_STATEMENT, case_by_id, placeholders,
};
use melodia_core::entities::track_search::FIELD_SEPARATOR;
use melodia_core::error::AppError;
use melodia_core::utils::fold::push_folded;

/// Rows per page, so that one page is one `UPDATE` inside the bind budget.
const PAGE_ROWS: usize = MAX_BINDS_PER_STATEMENT / CASE_BINDS_PER_ROW;

/// `fields` folded, each followed by [`FIELD_SEPARATOR`]. Terminated rather than joined, so a
/// one-field kind is never `''` once filled, `''` being how the backfill finds a row it owes.
pub(crate) fn pack(fields: &[&str]) -> String {
    let text_len: usize = fields.iter().map(|f| f.len()).sum();
    let mut packed = String::with_capacity(text_len + fields.len());
    for field in fields {
        push_folded(&mut packed, field);
        packed.push(FIELD_SEPARATOR);
    }
    packed
}

/// A table whose rows a grid filters, and the SELECT naming what each row packs.
#[derive(Clone, Copy)]
pub(crate) enum Named {
    /// The album's name and the artist line its card shows.
    Albums,
    /// The artist's name, and its sort name where it has one.
    Artists,
    Genres,
    Playlists,
}

impl Named {
    const ALL: [Self; 4] = [Self::Albums, Self::Artists, Self::Genres, Self::Playlists];

    fn table(self) -> &'static str {
        match self {
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Genres => "genres",
            Self::Playlists => "playlists",
        }
    }

    /// The inputs, as `id`, `name` and a nullable `extra`, from a source aliased `x`.
    fn inputs(self) -> &'static str {
        match self {
            Self::Albums => {
                "SELECT x.id AS id, x.name AS name, COALESCE(x.artist_credit, ar.name) AS extra \
                 FROM albums x JOIN artists ar ON ar.id = x.artist_id"
            }
            Self::Artists => {
                "SELECT x.id AS id, x.name AS name, x.sort_name AS extra FROM artists x"
            }
            Self::Genres => "SELECT x.id AS id, x.name AS name, NULL AS extra FROM genres x",
            Self::Playlists => "SELECT x.id AS id, x.name AS name, NULL AS extra FROM playlists x",
        }
    }
}

#[derive(sqlx::FromRow)]
struct Inputs {
    id: i64,
    name: String,
    extra: Option<String>,
}

impl Inputs {
    fn packed(&self) -> String {
        match &self.extra {
            Some(extra) => pack(&[&self.name, extra]),
            None => pack(&[&self.name]),
        }
    }
}

/// Writes `search_name` for the row `id` of `kind` from what that row holds now, for a writer
/// that changed one of its inputs without holding all of them.
pub(crate) async fn refill(
    conn: &mut SqliteConnection,
    kind: Named,
    id: i64,
) -> Result<(), AppError> {
    let sql = format!("{} WHERE x.id = ?", kind.inputs());
    let Some(row) =
        sqlx::query_as::<_, Inputs>(AssertSqlSafe(sql)).bind(id).fetch_optional(&mut *conn).await?
    else {
        return Ok(());
    };
    let sql = format!("UPDATE {} SET search_name = ? WHERE id = ?", kind.table());
    sqlx::query(AssertSqlSafe(sql)).bind(row.packed()).bind(id).execute(&mut *conn).await?;
    Ok(())
}

/// Fills `search_name` on every row of the four tables still holding `''`, a page at a time, and
/// returns how many rows it wrote. `backfill_search_text`'s shape and its reasons: cheap on a
/// filled library, one statement per page, and the `UPDATE` asking again that the column is
/// empty, so a row a writer filled meanwhile keeps the writer's value.
pub async fn backfill_search_name(db: &DbPool) -> Result<u64, AppError> {
    let mut written = 0;
    for kind in Named::ALL {
        written += backfill(db, kind).await?;
    }
    Ok(written)
}

async fn backfill(db: &DbPool, kind: Named) -> Result<u64, AppError> {
    let page_limit = i64::try_from(PAGE_ROWS).unwrap_or(i64::MAX);
    let sql =
        format!("{} WHERE x.search_name = '' AND x.id > ? ORDER BY x.id LIMIT ?", kind.inputs());
    let mut after_id = 0_i64;
    let mut written = 0;
    loop {
        let page: Vec<Inputs> = sqlx::query_as(AssertSqlSafe(sql.as_str()))
            .bind(after_id)
            .bind(page_limit)
            .fetch_all(db.read())
            .await?;
        let Some(last) = page.last() else {
            return Ok(written);
        };
        after_id = last.id;
        written += fill_page(db, kind, &page).await?;
    }
}

async fn fill_page(db: &DbPool, kind: Named, page: &[Inputs]) -> Result<u64, AppError> {
    let sql = format!(
        "UPDATE {} SET search_name = {} WHERE id IN ({}) AND search_name = ''",
        kind.table(),
        case_by_id(page.len()),
        placeholders(page.len()),
    );
    let mut query = sqlx::query(AssertSqlSafe(sql));
    for row in page {
        query = query.bind(row.id).bind(row.packed());
    }
    for row in page {
        query = query.bind(row.id);
    }
    Ok(query.persistent(false).execute(db.write()).await?.rows_affected())
}
