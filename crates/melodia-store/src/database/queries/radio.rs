use crate::database::queries::scan::to_natural_sort_key;
use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::radio::{self, StoredLogoAnswer};
use melodia_core::error::{AppError, describe};

/// The columns [`save_station`] writes, in bind order.
const INSERT_COLUMNS: &str = "station_uuid, name, stream_url, homepage, favicon_url, tags,
    country, country_code, language, codec, bitrate, hls, sort_key, date_added";

/// What a re-import may touch: the directory's own fields and nothing else. The user's side of the
/// row survives it (`is_favorite`, the play stats, `date_added`), and so does `artwork_path`, the
/// stored logo still being valid. The `local_*` columns are the user's answers to what the
/// directory left blank, and `probed_codec` is what an open measured, a better answer than the
/// container a `codec` names, so a re-import reverts neither.
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

/// Saves a station, resolving onto the existing row when the directory already knows its
/// `station_uuid`, and answers the row's id.
///
/// One door because the uuid already says which is wanted: a directory row always carries one and
/// a hand-typed URL never does, adding a row every time. `RETURNING id` rather than the row, which
/// every caller would drop; [`get_station_by_id`] reads back what `SQLite` stored.
pub async fn save_station(db: &DbPool, station: &radio::NewRadioStation) -> Result<i64, AppError> {
    save_station_on(db.write(), station).await
}

/// [`save_station`] against any executor, so the import can run its whole loop inside one
/// transaction without a second copy of the statement.
async fn save_station_on<'e, E>(
    executor: E,
    station: &radio::NewRadioStation,
) -> Result<i64, AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let sort_key = to_natural_sort_key(&station.name);
    let now = melodia_core::utils::now_rfc3339();
    let conflict = if station.station_uuid.is_some() { DIRECTORY_CONFLICT } else { "" };

    let sql = format!(
        "INSERT INTO radio_stations ({INSERT_COLUMNS})
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         {conflict}
         RETURNING id"
    );
    Ok(sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
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
        .fetch_one(executor)
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

/// The stations played most recently, newest first, ordered on the raw text: `now_rfc3339`
/// zero-pads its fraction and ends in `+00:00`, `+` sorting below every digit, so lexical order is
/// chronological even against a coarser stamp.
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

/// Rewrites the fields a hand-typed station's editor owns.
///
/// Not [`save_station`], which wants a uuid, country and language the caller has none of.
/// `artwork_path` is [`set_artwork`]'s, and `sort_key` is re-derived so a rename doesn't sort under
/// the old name.
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

/// The station already streaming from `stream_url`, if any: the duplicate guard both hand-typed
/// doors take. A query because a hand-typed station has no `station_uuid` for `UNIQUE` to catch;
/// the id rather than a bool because both doors merge onto the row they find.
pub async fn station_id_with_url(db: &DbPool, stream_url: &str) -> Result<Option<i64>, AppError> {
    Ok(sqlx::query_scalar("SELECT id FROM radio_stations WHERE stream_url = ? LIMIT 1")
        .bind(stream_url)
        .fetch_optional(db.read())
        .await?)
}

/// The row an import entry lands on, as `(id, is_favorite)`, or `None` for a new one.
///
/// The uuid first, the directory's identity for a station whose `stream_url` has moved, then the
/// URL, for an entry with no uuid or one typed in before the directory listed it. Executor-generic
/// because the import asks through its own transaction, which alone sees what earlier entries
/// wrote, and an undo through its own for the same reason.
pub(super) async fn kept_station_matching<'e, E>(
    executor: E,
    station_uuid: Option<&str>,
    stream_url: &str,
) -> Result<Option<(i64, bool)>, AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query_as::<_, (i64, bool)>(
        "SELECT id, is_favorite FROM radio_stations
         WHERE (? IS NOT NULL AND station_uuid = ?) OR stream_url = ?
         ORDER BY (station_uuid IS NOT NULL AND station_uuid = ?) DESC
         LIMIT 1",
    )
    .bind(station_uuid)
    .bind(station_uuid)
    .bind(stream_url)
    .bind(station_uuid)
    .fetch_optional(executor)
    .await?)
}

pub async fn set_favorite(db: &DbPool, id: i64, favorite: bool) -> Result<(), AppError> {
    set_favorite_on(db.write(), id, favorite).await
}

/// [`set_favorite`] against any executor. See [`save_station_on`].
async fn set_favorite_on<'e, E>(executor: E, id: i64, favorite: bool) -> Result<(), AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query("UPDATE radio_stations SET is_favorite = ? WHERE id = ?")
        .bind(favorite)
        .bind(id)
        .execute(executor)
        .await?;
    Ok(())
}

/// One entry of a station list on its way into the kept list.
pub struct ImportedStation {
    pub station: radio::NewRadioStation,
    /// The user's own fields the file carried, already validated; `None` where it carried none.
    pub overrides: Option<radio::StationOverrides>,
}

/// Puts every station in the kept list, answering how many entries that changed.
///
/// One transaction, so the list doesn't queue its statements one commit at a time behind other
/// writers, and so a station listed twice is favorited once. All-or-nothing on an error, which here
/// means an unwritable database, as [`super::artwork::repoint_all`] argues for its own pass.
pub async fn import_stations(db: &DbPool, stations: &[ImportedStation]) -> Result<u32, AppError> {
    let mut tx = db.write().begin().await?;
    let mut changed: u32 = 0;
    for entry in stations {
        if import_one(&mut tx, entry).await? {
            changed = changed.saturating_add(1);
        }
    }
    tx.commit().await?;
    Ok(changed)
}

/// Favorites one entry, answering whether that changed anything.
///
/// **A row already here takes only its favorite from the file**: [`set_local_fields`] writes all
/// four columns at once, so a file naming one would clear the three it says nothing about. Nothing
/// is lost, a deleted station arriving as a new row, and the rows overrides land on stay
/// logo-less, the state the logo repair needs.
async fn import_one(
    conn: &mut sqlx::SqliteConnection,
    entry: &ImportedStation,
) -> Result<bool, AppError> {
    let station = &entry.station;
    let existing =
        kept_station_matching(&mut *conn, station.station_uuid.as_deref(), &station.stream_url)
            .await?;

    let Some((id, is_favorite)) = existing else {
        let id = save_station_on(&mut *conn, station).await?;
        set_favorite_on(&mut *conn, id, true).await?;
        // Best-effort: one bad hand-edited line isn't worth refusing the list over, and every
        // field is editable on the card.
        if let Some(overrides) = &entry.overrides
            && let Err(e) = set_local_fields_on(&mut *conn, id, overrides).await
        {
            log::debug!("radio: imported details not stored: {}", describe(&e));
        }
        return Ok(true);
    };

    if is_favorite {
        return Ok(false);
    }
    set_favorite_on(&mut *conn, id, true).await?;
    Ok(true)
}

/// Records what a station's stream was measured to hold, which only a successful open knows.
/// Written on every tune rather than compared, a station that changed what it serves being the one
/// a stale value is wrong for.
pub async fn set_probed_codec(db: &DbPool, id: i64, codec: &str) -> Result<(), AppError> {
    sqlx::query("UPDATE radio_stations SET probed_codec = ? WHERE id = ?")
        .bind(codec)
        .bind(id)
        .execute(db.write())
        .await?;
    Ok(())
}

/// Count one play and stamp the time, the two columns the recents list reads.
pub async fn mark_played(db: &DbPool, id: i64) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE radio_stations SET play_count = play_count + 1, last_played = ? WHERE id = ?",
    )
    .bind(melodia_core::utils::now_rfc3339())
    .bind(id)
    .execute(db.write())
    .await?;
    Ok(())
}

/// Records what the *user* says about a station, clearing each field it carries `None` for.
///
/// The local columns only, the directory's four being rewritten by [`DIRECTORY_CONFLICT`], and the
/// only edit a directory-owned row accepts. One statement, so a save can't land half-applied.
pub async fn set_local_fields(
    db: &DbPool,
    id: i64,
    fields: &radio::StationOverrides,
) -> Result<(), AppError> {
    set_local_fields_on(db.write(), id, fields).await
}

/// [`set_local_fields`] against any executor. See [`save_station_on`].
async fn set_local_fields_on<'e, E>(
    executor: E,
    id: i64,
    fields: &radio::StationOverrides,
) -> Result<(), AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "UPDATE radio_stations
         SET local_homepage = ?, local_favicon_url = ?, local_tags = ?, local_country = ?
         WHERE id = ?",
    )
    .bind(fields.website.as_deref())
    .bind(fields.logo_url.as_deref())
    .bind(fields.genre.as_deref())
    .bind(fields.country.as_deref())
    .bind(id)
    .execute(executor)
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

/// What is already known about each of `favicon_urls`.
///
/// **Asked about a page, never the table**, which is keyed on URLs from a directory of tens of
/// thousands. One query learns both which logos a page has and which URLs it must not ask about
/// yet, the reason both live in one table.
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

/// Records that `favicon_url` answered with a file, and what it cost. Overwrites a miss, so a host
/// answering again isn't held to the backoff it earned while down.
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

/// Records that `favicon_url` answered with nothing, and when it may be asked again, the schedule
/// being `library::radio`'s. Clears any path the URL had, which would otherwise keep a file it no
/// longer serves.
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

/// Drops the answers no longer worth keeping, and reports how many went.
///
/// A **miss** past `miss_cutoff` would be retried anyway; a **hit** older than `hit_cutoff` is
/// cheaper to fetch again than to hold; and past `max_bytes` only the newest hits stay, the bound a
/// TTL alone can't give. Only rows go here, and the sweep retires the files no column names.
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

    // The running total is inclusive, so the row crossing the bound is the first dropped.
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
