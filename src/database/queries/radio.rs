use crate::database::queries::scan::to_natural_sort_key;
use crate::database::{DbPool, chunked_in_query};
use crate::entities::radio;
use crate::error::AppError;

/// The columns [`save_station`] writes, in bind order.
const INSERT_COLUMNS: &str = "station_uuid, name, stream_url, homepage, favicon_url, tags,
    country, country_code, language, codec, bitrate, hls, sort_key, date_added";

/// What a re-import is allowed to touch: the directory's own fields and nothing
/// else. `is_favorite`, `play_count`, `last_played` and `date_added` are the
/// user's side of the row and must survive it; `artwork_path` is left alone
/// because the stored logo is still valid and blanking it would strand the file
/// until the next sweep.
const DIRECTORY_CONFLICT: &str = "\
    ON CONFLICT(station_uuid) DO UPDATE SET
        name = excluded.name,
        stream_url = excluded.stream_url,
        homepage = excluded.homepage,
        favicon_url = excluded.favicon_url,
        tags = excluded.tags,
        country = excluded.country,
        country_code = excluded.country_code,
        language = excluded.language,
        codec = excluded.codec,
        bitrate = excluded.bitrate,
        hls = excluded.hls,
        sort_key = excluded.sort_key";

/// Save a station, resolving against the existing row when the directory
/// already knows its `station_uuid`.
///
/// One door rather than an upsert and an insert the caller picks between,
/// because the uuid already answers which is wanted: a directory row always
/// carries one and a hand-typed URL never does. The clause is dropped for a
/// `None` rather than left inert, `SQLite` treating NULLs as distinct under
/// UNIQUE, so a hand-typed station adds a row every time the way importing the
/// same playlist twice is two playlists.
pub async fn save_station(
    db: &DbPool,
    station: &radio::NewRadioStation,
) -> Result<radio::RadioStation, AppError> {
    let sort_key = to_natural_sort_key(&station.name);
    let now = crate::utils::now_rfc3339();
    let conflict = if station.station_uuid.is_some() {
        DIRECTORY_CONFLICT
    } else {
        ""
    };

    let sql = format!(
        "INSERT INTO radio_stations ({INSERT_COLUMNS})
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         {conflict}
         RETURNING *"
    );
    Ok(sqlx::query_as::<_, radio::RadioStation>(sqlx::AssertSqlSafe(sql))
        .bind(&station.station_uuid)
        .bind(&station.name)
        .bind(&station.stream_url)
        .bind(&station.homepage)
        .bind(&station.favicon_url)
        .bind(&station.tags)
        .bind(&station.country)
        .bind(&station.country_code)
        .bind(&station.language)
        .bind(&station.codec)
        .bind(station.bitrate)
        .bind(station.hls)
        .bind(&sort_key)
        .bind(&now)
        .fetch_one(db.write())
        .await?)
}

/// One station, or `AppError::NotFound` if it was deleted between a list render
/// and the click.
pub async fn get_station_by_id(db: &DbPool, id: i64) -> Result<radio::RadioStation, AppError> {
    sqlx::query_as::<_, radio::RadioStation>("SELECT * FROM radio_stations WHERE id = ?")
        .bind(id)
        .fetch_optional(db.read())
        .await?
        .ok_or_else(|| AppError::not_found("Radio station", id))
}

/// Every favorited station, naturally name-ordered.
pub async fn get_favorite_stations(db: &DbPool) -> Result<Vec<radio::RadioStation>, AppError> {
    Ok(sqlx::query_as::<_, radio::RadioStation>(
        "SELECT * FROM radio_stations WHERE is_favorite = TRUE
         ORDER BY sort_key COLLATE NOCASE ASC",
    )
    .fetch_all(db.read())
    .await?)
}

/// The stations played most recently, newest first.
///
/// Ordered on the raw `TEXT`. `now_rfc3339` zero-pads its fraction and closes
/// with a constant `+00:00`, and `+` sorts below every digit, so a stamp printed
/// at coarser precision still lands ahead of a finer one sharing its prefix:
/// lexical order is chronological order and nothing is parsed.
pub async fn get_recent_stations(
    db: &DbPool,
    limit: i64,
) -> Result<Vec<radio::RadioStation>, AppError> {
    Ok(sqlx::query_as::<_, radio::RadioStation>(
        "SELECT * FROM radio_stations WHERE last_played IS NOT NULL
         ORDER BY last_played DESC
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(db.read())
    .await?)
}

/// Rewrite the fields a hand-typed station's editor owns.
///
/// Narrower than [`save_station`] on purpose: that one is the directory's door
/// and would need a `NewRadioStation` the caller has no uuid, country or
/// language for, and it would reset the play stats through `RETURNING *`.
/// `artwork_path` stays out too — the logo is a file the caller fetches, so it
/// is `set_artwork`'s to point at. `sort_key` is re-derived here because it is
/// the name's shadow and a rename that left it standing would sort the station
/// under its old one.
pub async fn update_station(
    db: &DbPool,
    id: i64,
    edit: &radio::StationEdit,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE radio_stations
         SET name = ?, stream_url = ?, homepage = ?, favicon_url = ?, tags = ?,
             codec = ?, bitrate = ?, sort_key = ?
         WHERE id = ?",
    )
    .bind(&edit.name)
    .bind(&edit.stream_url)
    .bind(&edit.homepage)
    .bind(&edit.favicon_url)
    .bind(&edit.tags)
    .bind(&edit.codec)
    .bind(edit.bitrate)
    .bind(to_natural_sort_key(&edit.name))
    .bind(id)
    .execute(db.write())
    .await?;
    Ok(())
}

/// The station already streaming from `stream_url`, if there is one.
///
/// The duplicate guard both hand-typed doors take, and it has to be a query: a
/// hand-typed station carries no `station_uuid`, and `SQLite` treats NULLs as
/// distinct under `UNIQUE`, so the constraint that stops a directory station
/// arriving twice says nothing at all about this one. The id comes back rather
/// than a bool because the add merges onto the row it finds; the import only
/// asks whether there was one.
pub async fn station_id_with_url(db: &DbPool, stream_url: &str) -> Result<Option<i64>, AppError> {
    Ok(sqlx::query_scalar("SELECT id FROM radio_stations WHERE stream_url = ? LIMIT 1")
        .bind(stream_url)
        .fetch_optional(db.read())
        .await?)
}

pub async fn set_favorite(db: &DbPool, id: i64, favorite: bool) -> Result<(), AppError> {
    sqlx::query("UPDATE radio_stations SET is_favorite = ? WHERE id = ?")
        .bind(favorite)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

pub async fn delete_station(db: &DbPool, id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM radio_stations WHERE id = ?").bind(id).execute(db.write()).await?;
    Ok(())
}

/// Count one play and stamp the time, the two columns the recents list reads.
pub async fn mark_played(db: &DbPool, id: i64) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE radio_stations SET play_count = play_count + 1, last_played = ? WHERE id = ?",
    )
    .bind(crate::utils::now_rfc3339())
    .bind(id)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Point a station at its stored logo, or clear it with `None`.
pub async fn set_artwork(db: &DbPool, id: i64, artwork_path: Option<&str>) -> Result<(), AppError> {
    sqlx::query("UPDATE radio_stations SET artwork_path = ? WHERE id = ?")
        .bind(artwork_path)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// Which of `favicon_urls` are still inside their backoff at `now`, so a browse can skip asking.
///
/// **Asked about a page, never about the table.** The key is a URL from a directory of tens of
/// thousands of entries, a third of which carry a logo field that answers with nothing, so the row
/// count has no bound a caller could read whole into a memo that holds two thousand.
///
/// The clock comparison lands in Rust rather than in the `WHERE`, since the placeholder list is
/// what `chunked_in_query` binds and a second parameter would have to ride ahead of it. Still a
/// string comparison against the same `to_rfc3339` shape both sides are written in.
pub async fn suppressed_logo_urls(
    db: &DbPool,
    favicon_urls: &[String],
    now: &str,
) -> Result<Vec<String>, AppError> {
    let scheduled: Vec<(String, String)> =
        chunked_in_query(db.read(), favicon_urls, |placeholders| {
            format!(
                "SELECT favicon_url, retry_after FROM radio_logo_misses \
                 WHERE favicon_url IN ({placeholders})"
            )
        })
        .await?;

    Ok(scheduled
        .into_iter()
        .filter(|(_, retry_after)| retry_after.as_str() > now)
        .map(|(url, _)| url)
        .collect())
}

/// How many times `favicon_url` has already answered with nothing, or `None` if never.
pub async fn logo_miss_attempts(db: &DbPool, favicon_url: &str) -> Result<Option<i64>, AppError> {
    let attempts =
        sqlx::query_scalar("SELECT attempts FROM radio_logo_misses WHERE favicon_url = ?")
            .bind(favicon_url)
            .fetch_optional(db.read())
            .await?;
    Ok(attempts)
}

/// Record that `favicon_url` answered with nothing, and when it may be asked again.
///
/// Both values are the caller's: [`logo_miss_attempts`] is what it counted from, and the schedule
/// is `library::radio`'s to decide.
pub async fn upsert_logo_miss(
    db: &DbPool,
    favicon_url: &str,
    attempts: i64,
    retry_after: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO radio_logo_misses (favicon_url, attempts, retry_after) VALUES (?, ?, ?)
         ON CONFLICT(favicon_url) DO UPDATE SET
             attempts = excluded.attempts,
             retry_after = excluded.retry_after",
    )
    .bind(favicon_url)
    .bind(attempts)
    .bind(retry_after)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Forget `favicon_url`'s misses, for a host that has started answering again — otherwise a URL
/// that failed twice and then recovered stays suppressed until its old backoff runs out.
pub async fn clear_logo_miss(db: &DbPool, favicon_url: &str) -> Result<(), AppError> {
    sqlx::query("DELETE FROM radio_logo_misses WHERE favicon_url = ?")
        .bind(favicon_url)
        .execute(db.write())
        .await?;
    Ok(())
}

/// Drop misses whose retry time passed before `cutoff`, which is what bounds the table.
///
/// A row that far past its own backoff has nothing left to say: it would be retried on the next
/// page carrying it either way, and keeping it only makes the load above larger.
pub async fn prune_logo_misses(db: &DbPool, cutoff: &str) -> Result<(), AppError> {
    sqlx::query("DELETE FROM radio_logo_misses WHERE retry_after < ?")
        .bind(cutoff)
        .execute(db.write())
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
