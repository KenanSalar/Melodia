//! Track-row write side: insert (single + multi-row), metadata refresh,
//! location-only update (for moved/renamed files), bulk delete, and the
//! album-artwork roll-up that runs at the tail of every scan batch.

use std::collections::HashMap;

use sqlx::{AssertSqlSafe, Row};

use crate::database::{SQLITE_BIND_LIMIT, placeholders};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

use super::ResolvedIds;
use super::sort_key::to_natural_sort_key;
use super::upserts::{insert_track_joins, replace_track_joins};

/// Column-name list shared by [`insert_track`]'s single-row INSERT and
/// [`insert_tracks_batch`]'s multi-row form.
///
/// Both reach it through the one [`bind_track_columns`], and the placeholder grid and the bind
/// budget below are counted off this string rather than typed out, so adding a column here is the
/// whole edit. `update_track_metadata` binds through the same helper, which is what couples its
/// SET list to this order.
const TRACK_INSERT_COLUMNS: &str = "file_path, file_name,
    file_hash, title, artist, album_artist, album, genre, credits,
    track_number, track_total, disc_number, disc_total, disc_subtitle, subtitle,
    release_date, year, original_date, original_year, comment,
    bpm, initial_key, mood, grouping, work, movement, movement_number, movement_total,
    language, copyright, isrc,
    musicbrainz_track_id, musicbrainz_release_id, musicbrainz_release_track_id,
    replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak,
    duration_ms, file_size, codec, bitrate, channels, sample_rate, bit_depth,
    artwork_path,
    album_id, artist_id, genre_id, folder_id,
    date_modified, sort_key,
    play_count, skip_count, rating, is_favorite, last_played, last_position,
    date_added";

/// How many names a comma-separated column list holds. `const` so everything sized off
/// [`TRACK_INSERT_COLUMNS`] is derived from it rather than counted by hand and left to drift.
const fn column_count(list: &str) -> usize {
    let bytes = list.as_bytes();
    let mut count = 1;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b',' {
            count += 1;
        }
        i += 1;
    }
    count
}

const TRACK_INSERT_COLUMN_COUNT: usize = column_count(TRACK_INSERT_COLUMNS);

/// The playback defaults pushed as SQL literals rather than bound — `play_count`, `skip_count`,
/// `is_favorite`, `last_played`, `last_position`. They carry nothing from the file, and the
/// rating beside them binds because it does.
const LITERAL_DEFAULTS: usize = 5;

/// Update a track's artwork if it is currently missing.
pub async fn update_track_artwork_if_missing(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    artwork_path: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE tracks SET artwork_path = ?
         WHERE file_path = ? AND (artwork_path IS NULL OR artwork_path = '')",
    )
    .bind(artwork_path)
    .bind(file_path)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Somewhere a track's columns can be bound, in order.
///
/// The single-row INSERT chains `Query::bind` and the multi-row one chains `Separated::push_bind`,
/// which are the same operation through two types sqlx shares no trait between. One trait over
/// both is what lets [`bind_track_columns`] be the only place the order is written down: a second
/// copy is fifty adjacent binds that a reorder between two same-typed nullable columns would move
/// silently, since arity still checks out and every value still encodes.
trait TrackBinder<'q> {
    fn bind_col<T: sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>>(
        self,
        value: T,
    ) -> Self;
}

impl<'q> TrackBinder<'q> for sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
    fn bind_col<T: sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>>(
        self,
        value: T,
    ) -> Self {
        self.bind(value)
    }
}

impl<'q, Sep: std::fmt::Display> TrackBinder<'q>
    for &mut sqlx::query_builder::Separated<'_, sqlx::Sqlite, Sep>
{
    fn bind_col<T: sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>>(
        self,
        value: T,
    ) -> Self {
        self.push_bind(value);
        self
    }
}

/// Bind the canonical 50-column block shared by `insert_track`,
/// `insert_tracks_batch` and `update_track_metadata`: 44 metadata fields
/// (`file_hash` through `artwork_path`), 4 foreign-key ids (`album_id`, `artist_id`, `genre_id`,
/// `folder_id`), then `date_modified` and `sort_key`. All three call sites place
/// these in the same order in their respective SQL so this single helper
/// covers schema changes in one place.
fn bind_track_columns<'q, B: TrackBinder<'q>>(
    binder: B,
    meta: &'q melodia_core::entities::scan::ExtractedMetadata,
    ids: &'q ResolvedIds,
    sort_key: &'q str,
) -> B {
    binder
        .bind_col(&meta.file_hash)
        .bind_col(&meta.title)
        .bind_col(meta.artist.line())
        .bind_col(meta.album_artist.line())
        .bind_col(&meta.album)
        .bind_col(meta.genres.line())
        .bind_col(meta.credits.line())
        .bind_col(meta.track_number)
        .bind_col(meta.track_total)
        .bind_col(meta.disc_number)
        .bind_col(meta.disc_total)
        .bind_col(&meta.disc_subtitle)
        .bind_col(&meta.subtitle)
        .bind_col(&meta.release_date)
        .bind_col(meta.year)
        .bind_col(&meta.original_date)
        .bind_col(meta.original_year)
        .bind_col(&meta.comment)
        .bind_col(meta.bpm)
        .bind_col(&meta.initial_key)
        .bind_col(&meta.mood)
        .bind_col(&meta.grouping)
        .bind_col(&meta.work)
        .bind_col(&meta.movement)
        .bind_col(meta.movement_number)
        .bind_col(meta.movement_total)
        .bind_col(&meta.language)
        .bind_col(&meta.copyright)
        .bind_col(&meta.isrc)
        .bind_col(&meta.musicbrainz_track_id)
        .bind_col(&meta.musicbrainz_release_id)
        .bind_col(&meta.musicbrainz_release_track_id)
        .bind_col(meta.replaygain_track_gain)
        .bind_col(meta.replaygain_track_peak)
        .bind_col(meta.replaygain_album_gain)
        .bind_col(meta.replaygain_album_peak)
        .bind_col(meta.duration_ms)
        .bind_col(Some(meta.file_size))
        .bind_col(&meta.codec)
        .bind_col(meta.bitrate)
        .bind_col(meta.channels)
        .bind_col(meta.sample_rate)
        .bind_col(meta.bit_depth)
        .bind_col(&meta.artwork_path)
        .bind_col(ids.album_id)
        .bind_col(ids.artist_id)
        .bind_col(ids.genre_id)
        .bind_col(ids.folder_id)
        .bind_col(&meta.date_modified)
        .bind_col(sort_key)
}

/// The string a track's `sort_key` is derived from: the file's `TITLESORT` where it has one, the
/// title otherwise. Spelled once because `insert_track`, `insert_tracks_batch` and
/// `update_track_metadata` all need it and a row sorting by a different rule than its neighbours
/// is invisible until a list is read top to bottom.
fn sort_source(meta: &ExtractedMetadata) -> &str {
    meta.sort.title.as_deref().unwrap_or(&meta.title)
}

/// Insert a new track into the database. Returns the new track's `id` so the
/// caller can collect inserted IDs without a follow-up `SELECT … WHERE
/// file_path IN (…)` round-trip.
/// `now` should be a pre-computed RFC3339 timestamp so all tracks in a batch share the same `date_added`.
pub async fn insert_track(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    file_name: &str,
    meta: &melodia_core::entities::scan::ExtractedMetadata,
    ids: &ResolvedIds,
    now: &str,
) -> Result<i64, AppError> {
    let sort_key = to_natural_sort_key(sort_source(meta));

    // Column order: file_path, file_name, then the 50-column shared block
    // bound by `bind_track_columns`, then the playback defaults and date_added.
    let sql = format!(
        "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) VALUES ({})",
        placeholders(TRACK_INSERT_COLUMN_COUNT)
    );
    let q = sqlx::query(AssertSqlSafe(sql)).bind(file_path).bind(file_name);

    let q = bind_track_columns(q, meta, ids, &sort_key);

    let result = q
        .bind(0i32) // play_count
        .bind(0i32) // skip_count
        .bind(meta.rating.unwrap_or(0)) // rating — seeded from the file's own tag
        .bind(false) // is_favorite
        .bind(None::<String>) // last_played
        .bind(0i64) // last_position
        .bind(now) // date_added
        .execute(&mut **tx)
        .await?;
    let track_id = result.last_insert_rowid();
    insert_track_joins(tx, track_id, meta, ids).await?;
    Ok(track_id)
}

/// One buffered row for [`insert_tracks_batch`]. `meta` borrows from the
/// scanned-file set the ingest loop iterates; the resolved ids are moved in.
pub struct NewTrackRow<'a> {
    pub file_path: String,
    pub file_name: String,
    pub meta: &'a ExtractedMetadata,
    pub ids: ResolvedIds,
}

/// Rows per multi-row INSERT statement, sized so a chunk stays under `SQLite`'s bind cap. A row
/// binds `file_path`, `file_name`, the shared block, `rating` and `date_added`; the five playback
/// defaults beside them ride as SQL literals, which is what [`LITERAL_DEFAULTS`] subtracts.
///
/// The rating is bound rather than pushed as a literal like its neighbours
/// because it is the one of the six that carries a *value* — the file's own
/// tag — and data never rides in the statement text.
pub const INSERT_CHUNK_ROWS: usize =
    SQLITE_BIND_LIMIT / (TRACK_INSERT_COLUMN_COUNT - LITERAL_DEFAULTS);

/// Multi-row variant of [`insert_track`] for the scan/import ingest hot
/// path: one `INSERT … VALUES (…), (…), … RETURNING id, file_path` per
/// [`INSERT_CHUNK_ROWS`] rows instead of one statement per row. Returns
/// the new ids **in `rows` order** — `SQLite` documents `RETURNING`
/// output order as unspecified, so ids are mapped back via the returned
/// `file_path` (a UNIQUE column) rather than by position. Callers rely
/// on that ordering: the `DnD` import queues tracks in drop order.
pub async fn insert_tracks_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    rows: &[NewTrackRow<'_>],
    now: &str,
) -> Result<Vec<i64>, AppError> {
    let mut out = Vec::with_capacity(rows.len());
    for chunk in rows.chunks(INSERT_CHUNK_ROWS) {
        let sort_keys: Vec<String> =
            chunk.iter().map(|r| to_natural_sort_key(sort_source(r.meta))).collect();

        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(format!(
            "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) "
        ));
        qb.push_values(chunk.iter().zip(sort_keys.iter()), |mut b, (row, sort_key)| {
            let meta = row.meta;
            b.push_bind(&row.file_path).push_bind(&row.file_name);
            bind_track_columns(&mut b, meta, &row.ids, sort_key);
            // Playback defaults as SQL literals (play_count, skip_count,
            // is_favorite, last_played, last_position); the rating comes off
            // the file's tag, so it binds.
            b.push("0")
                .push("0")
                .push_bind(meta.rating.unwrap_or(0))
                .push("0")
                .push("NULL")
                .push("0")
                .push_bind(now);
        });
        qb.push(" RETURNING id, file_path");

        let returned: Vec<(i64, String)> =
            qb.build_query_as().persistent(false).fetch_all(&mut **tx).await?;
        let mut by_path: HashMap<String, i64> =
            returned.into_iter().map(|(id, path)| (path, id)).collect();
        for row in chunk {
            let Some(id) = by_path.remove(row.file_path.as_str()) else {
                // Unreachable in practice — every VALUES row produces a
                // RETURNING row — but fail loudly rather than silently
                // desync `inserted_track_ids` from reality.
                return Err(AppError::scanner_msg(format!(
                    "multi-row insert returned no id for {}",
                    row.file_path
                )));
            };
            insert_track_joins(tx, id, row.meta, &row.ids).await?;
            out.push(id);
        }
    }
    Ok(out)
}

/// Update a track's location when it has been moved or renamed.
/// Preserves all playback state (`play_count`, rating, `is_favorite`, etc.).
/// Returns `false` when no row with `track_id` exists — callers holding a
/// candidate id resolved *before* the transaction opened must treat that
/// as "the row vanished in-tx" and fall back to inserting.
pub async fn update_track_location(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    new_path: &str,
    new_file_name: &str,
    new_folder_id: i64,
    date_modified: Option<&str>,
) -> Result<bool, AppError> {
    let result = sqlx::query(
        "UPDATE tracks SET file_path = ?, file_name = ?, folder_id = ?, date_modified = ?
         WHERE id = ?",
    )
    .bind(new_path)
    .bind(new_file_name)
    .bind(new_folder_id)
    .bind(date_modified)
    .bind(track_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Update all metadata columns for an existing track that has changed on disk.
/// Preserves playback state (`play_count`, `is_favorite`, `last_played`, `last_position`).
///
/// The rating is the exception, and it is a tag-backed field like any other: retag a title
/// elsewhere and a rescan shows the new title, so a star set elsewhere has to land the same way.
/// What stops that being a plain `rating = ?` is the track whose file cannot carry one — a
/// filename-derived Matroska row, a file on read-only media, an install with the write-back
/// turned off — where the row is the only copy there is. Hence the `CASE`: the tag wins whenever
/// it says something, and silence leaves the row alone.
///
/// The tag winning is not confined to the write-back's own re-extract: this runs on every rescan
/// and every Edit-Tags save. So with the write-back **off**, a star set here is replaced by
/// whatever another player wrote, and a star *cleared* here comes back, the file still carrying
/// the tag and a cleared row being indistinguishable from an untouched one. Deliberate, on the
/// same reading of "tag-backed field" as the title above it, and the price of the column not
/// being able to say "the user meant zero".
pub async fn update_track_metadata(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    meta: &melodia_core::entities::scan::ExtractedMetadata,
    ids: &ResolvedIds,
) -> Result<(), AppError> {
    let sort_key = to_natural_sort_key(sort_source(meta));
    let q = sqlx::query(
        "UPDATE tracks SET
            file_hash = ?, title = ?, artist = ?, album_artist = ?, album = ?,
            genre = ?, credits = ?,
            track_number = ?, track_total = ?, disc_number = ?, disc_total = ?,
            disc_subtitle = ?, subtitle = ?,
            release_date = ?, year = ?, original_date = ?, original_year = ?, comment = ?,
            bpm = ?, initial_key = ?, mood = ?, grouping = ?,
            work = ?, movement = ?, movement_number = ?, movement_total = ?,
            language = ?, copyright = ?, isrc = ?,
            musicbrainz_track_id = ?, musicbrainz_release_id = ?,
            musicbrainz_release_track_id = ?,
            replaygain_track_gain = ?, replaygain_track_peak = ?,
            replaygain_album_gain = ?, replaygain_album_peak = ?,
            duration_ms = ?, file_size = ?, codec = ?, bitrate = ?,
            channels = ?, sample_rate = ?, bit_depth = ?,
            artwork_path = COALESCE(?, artwork_path),
            album_id = ?, artist_id = ?, genre_id = ?, folder_id = ?,
            date_modified = ?, sort_key = ?,
            rating = CASE WHEN ? = 0 THEN rating ELSE ? END
         WHERE file_path = ? RETURNING id",
    );
    let q = bind_track_columns(q, meta, ids, &sort_key);
    let tag_rating = meta.rating.unwrap_or(0);
    let updated =
        q.bind(tag_rating).bind(tag_rating).bind(file_path).fetch_optional(&mut **tx).await?;

    // Here rather than at the four callers, for the reason the module doc gives about hand-built
    // UPDATEs: a re-ingest that refreshed the artist column and left the credit rows behind is a
    // track that displays one thing and files under another. `RETURNING` rather than a follow-up
    // SELECT, so an incremental scan pays no extra round trip per changed file.
    if let Some(row) = updated {
        let track_id: i64 = row.try_get("id")?;
        replace_track_joins(tx, track_id, meta, ids).await?;
    }
    Ok(())
}

/// Update albums that are missing artwork by pulling from their tracks.
/// Uses a CTE to calculate first artwork per album in a single pass,
/// avoiding a correlated subquery per album.
pub async fn update_album_artwork_from_tracks(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    sqlx::query(
        "WITH first_art AS (
            SELECT album_id, artwork_path,
                   ROW_NUMBER() OVER (PARTITION BY album_id ORDER BY id) AS rn
            FROM tracks
            WHERE album_id IS NOT NULL
              AND artwork_path IS NOT NULL
              AND artwork_path != ''
        )
        UPDATE albums SET artwork_path = (
            SELECT artwork_path FROM first_art
            WHERE first_art.album_id = albums.id AND rn = 1
        )
        WHERE (artwork_path IS NULL OR artwork_path = '')
          AND id IN (SELECT album_id FROM first_art WHERE rn = 1)",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Delete album, artist, and genre rows left with no tracks — orphans stranded
/// by a retag that moved a track to a different album/genre, or by a track
/// deletion that removed a parent's last member. Nothing else prunes these: the
/// stats triggers decrement counts on the emptied row but never delete it.
///
/// Order matters — albums first, so an artist whose only album just emptied
/// becomes prunable in the same pass; then artists, except the id-1 "unknown"
/// default that `albums.artist_id` falls back to. `artists.album_count` is
/// recomputed afterwards as a backstop rather than because nothing maintains
/// it: `album_artists_stats_delete` moves it per row, and the album DELETE
/// above reaches that through `albums_credits_cleanup`. Genres share no FK with
/// albums/artists and have no sentinel, so they're pruned independently.
///
/// **The credit tables are part of what keeps an artist alive**, and they are the only thing
/// keeping a featured-only one: nothing points at them from `tracks.artist_id`, and
/// `track_artists.artist_id` cascades. So a predicate over the two FKs alone deletes exactly the
/// artists the credit tables exist to surface, and takes their credit rows down with them.
pub async fn prune_orphans(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM albums \
         WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.album_id = albums.id)",
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "DELETE FROM artists \
         WHERE id <> 1 \
           AND NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM albums WHERE albums.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM track_artists WHERE track_artists.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM album_artists WHERE album_artists.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM track_credits WHERE track_credits.artist_id = artists.id)",
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "DELETE FROM genres \
         WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.genre_id = genres.id) \
           AND NOT EXISTS (SELECT 1 FROM track_genres WHERE track_genres.genre_id = genres.id)",
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "UPDATE artists \
         SET album_count = \
             (SELECT COUNT(*) FROM album_artists WHERE album_artists.artist_id = artists.id)",
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Delete a single track by file path.
/// The `tracks_stats_delete` trigger fires automatically, keeping stats correct.
pub async fn delete_track_by_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM tracks WHERE file_path = ?")
        .bind(file_path)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Batch-delete tracks by file paths, respecting `SQLite`'s 999-parameter bind limit.
/// Returns the total number of rows deleted.
pub async fn delete_tracks_by_paths_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_paths: &[String],
) -> Result<u64, AppError> {
    if file_paths.is_empty() {
        return Ok(0);
    }

    let mut total_deleted: u64 = 0;
    for chunk in file_paths.chunks(crate::database::SQLITE_BIND_LIMIT) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!("DELETE FROM tracks WHERE file_path IN ({placeholders})");
        let mut query = sqlx::query(AssertSqlSafe(sql));
        for path in chunk {
            query = query.bind(path);
        }
        let result = query.persistent(false).execute(&mut **tx).await?;
        total_deleted += result.rows_affected();
    }
    Ok(total_deleted)
}
