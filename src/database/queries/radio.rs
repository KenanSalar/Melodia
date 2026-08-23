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

/// Forget a station's plays, which is what drops it out of the recents list.
///
/// The count goes with the stamp: Favorites sorts on it, and a station showing seven plays and no
/// last-played is a history half-erased.
pub async fn clear_play_history(db: &DbPool, id: i64) -> Result<(), AppError> {
    sqlx::query("UPDATE radio_stations SET last_played = NULL, play_count = 0 WHERE id = ?")
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

/// What an earlier session's attempt at one logo URL left behind.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredLogoAnswer {
    pub favicon_url: String,
    /// The stored file, or `None` where the URL answered with nothing.
    pub artwork_path: Option<String>,
    /// When this URL may be asked again. `None` on a hit.
    pub retry_after: Option<String>,
}

/// What is already known about each of `favicon_urls`.
///
/// **Asked about a page, never about the table.** The key is a URL from a directory of tens of
/// thousands of entries, so the row count has no bound a caller could read whole into a memo that
/// holds two thousand.
///
/// One query for both halves of the answer, which is the whole reason the two live in one table:
/// a page of fifty stations asks once and learns both which logos it already has and which URLs
/// it must not ask about yet.
pub async fn logo_answers(
    db: &DbPool,
    favicon_urls: &[String],
) -> Result<Vec<StoredLogoAnswer>, AppError> {
    chunked_in_query(db.read(), favicon_urls, |placeholders| {
        format!(
            "SELECT favicon_url, artwork_path, retry_after FROM radio_logo_answers \
             WHERE favicon_url IN ({placeholders})"
        )
    })
    .await
}

/// How many times `favicon_url` has already answered with nothing.
pub async fn logo_miss_attempts(db: &DbPool, favicon_url: &str) -> Result<Option<i64>, AppError> {
    let attempts =
        sqlx::query_scalar("SELECT attempts FROM radio_logo_answers WHERE favicon_url = ?")
            .bind(favicon_url)
            .fetch_optional(db.read())
            .await?;
    Ok(attempts)
}

/// Record that `favicon_url` answered with a file, and what it cost.
///
/// Overwrites a miss rather than deleting one: a host that has started answering again must not
/// stay suppressed by a backoff it earned while it was down, and the row is now the hit.
pub async fn record_logo_hit(
    db: &DbPool,
    favicon_url: &str,
    artwork_path: &str,
    bytes: i64,
    answered_at: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO radio_logo_answers
             (favicon_url, artwork_path, bytes, attempts, retry_after, answered_at)
         VALUES (?, ?, ?, 0, NULL, ?)
         ON CONFLICT(favicon_url) DO UPDATE SET
             artwork_path = excluded.artwork_path,
             bytes = excluded.bytes,
             attempts = 0,
             retry_after = NULL,
             answered_at = excluded.answered_at",
    )
    .bind(favicon_url)
    .bind(artwork_path)
    .bind(bytes)
    .bind(answered_at)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Record that `favicon_url` answered with nothing, and when it may be asked again.
///
/// Both values are the caller's: [`logo_miss_attempts`] is what it counted from, and the schedule
/// is `library::radio`'s to decide. Clears any path the URL used to have — the logo moved or went
/// away, and a row naming a file this URL no longer serves would hold it on disk forever.
pub async fn record_logo_miss(
    db: &DbPool,
    favicon_url: &str,
    attempts: i64,
    retry_after: &str,
    answered_at: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO radio_logo_answers
             (favicon_url, artwork_path, bytes, attempts, retry_after, answered_at)
         VALUES (?, NULL, 0, ?, ?, ?)
         ON CONFLICT(favicon_url) DO UPDATE SET
             artwork_path = NULL,
             bytes = 0,
             attempts = excluded.attempts,
             retry_after = excluded.retry_after,
             answered_at = excluded.answered_at",
    )
    .bind(favicon_url)
    .bind(attempts)
    .bind(retry_after)
    .bind(answered_at)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Drop the answers no longer worth keeping, and report how many went.
///
/// Three rules, and the caller owns all three numbers. A **miss** past `miss_cutoff` has nothing
/// left to say — it would be retried on the next page carrying it either way. A **hit** older than
/// `hit_cutoff` is a logo nobody has looked at in long enough that re-fetching it once is cheaper
/// than holding it. And past `max_bytes` the newest hits are kept and the rest go, which is the
/// bound that actually holds: a TTL alone lets a heavy browsing habit run the store up as far as
/// its own rate takes it.
///
/// **The rows go and the files follow.** Nothing here touches the store — a dropped row simply
/// stops referencing its file, and the sweep retires whatever no column names.
pub async fn prune_logo_answers(
    db: &DbPool,
    miss_cutoff: &str,
    hit_cutoff: &str,
    max_bytes: i64,
) -> Result<u64, AppError> {
    let stale = sqlx::query(
        "DELETE FROM radio_logo_answers
         WHERE (artwork_path IS NULL AND retry_after < ?)
            OR (artwork_path IS NOT NULL AND answered_at < ?)",
    )
    .bind(miss_cutoff)
    .bind(hit_cutoff)
    .execute(db.write())
    .await?
    .rows_affected();

    // Newest-first, so what survives is what a browse is most likely to ask for next. The running
    // total is inclusive, hence `>`: the row that crosses the bound is the first one dropped.
    let over_cap = sqlx::query(
        "DELETE FROM radio_logo_answers WHERE favicon_url IN (
             SELECT favicon_url FROM (
                 SELECT favicon_url,
                        SUM(bytes) OVER (ORDER BY answered_at DESC, favicon_url DESC) AS running
                 FROM radio_logo_answers
                 WHERE artwork_path IS NOT NULL
             ) WHERE running > ?
         )",
    )
    .bind(max_bytes)
    .execute(db.write())
    .await?
    .rows_affected();

    Ok(stale + over_cap)
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
