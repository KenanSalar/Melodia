//! The track rows' write side, and the cover roll-ups and orphan prune at the tail of a batch.

use std::collections::HashMap;

use sqlx::{AssertSqlSafe, Row};

use crate::database::{MAX_BINDS_PER_STATEMENT, placeholders};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::track_search::SearchFields;
use melodia_core::error::AppError;

use super::ResolvedIds;
use super::name_cache::NameCache;
use super::sort_key::to_natural_sort_key;
use super::upserts::{insert_track_joins, replace_track_joins};

/// The columns [`insert_track`] and [`insert_tracks_batch`] write, in the order
/// [`bind_track_columns`] binds them, which `update_track_metadata`'s `SET` list follows too. The
/// placeholder grid and the bind budget are counted off this string.
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
    date_modified, sort_key, search_text,
    play_count, skip_count, rating, is_favorite, last_played, last_position,
    date_added";

/// How many names a comma-separated column list holds, at compile time.
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

/// The playback defaults pushed as SQL literals: `play_count`, `skip_count`, `is_favorite`,
/// `last_played`, `last_position`. The rating beside them binds, carrying the file's tag.
const LITERAL_DEFAULTS: usize = 5;

/// Somewhere a track's columns can be bound, in order: `Query::bind` and `Separated::push_bind`,
/// which sqlx shares no trait between. One trait keeps [`bind_track_columns`] the only place the
/// order is written, where a second copy could swap two same-typed nullable columns silently.
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

/// Binds the block every track write shares: the metadata from `file_hash` to `artwork_path`, the
/// four foreign keys, `date_modified` and the two [`DerivedColumns`].
fn bind_track_columns<'q, B: TrackBinder<'q>>(
    binder: B,
    meta: &'q melodia_core::entities::scan::ExtractedMetadata,
    ids: &'q ResolvedIds,
    derived: &'q DerivedColumns,
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
        .bind_col(&derived.sort_key)
        .bind_col(&derived.search_text)
}

/// The two columns computed from a file's tags rather than read off them, one rule for every
/// write: a row sorting or filtering differently from its neighbours is invisible until a list is
/// read top to bottom.
struct DerivedColumns {
    sort_key: String,
    search_text: String,
}

impl DerivedColumns {
    fn of(meta: &ExtractedMetadata) -> Self {
        let sort_source = meta.sort.title.as_deref().unwrap_or(&meta.title);
        let search_text = SearchFields {
            title: &meta.title,
            artist: meta.artist.line().unwrap_or_default(),
            album_artist: meta.album_artist.line().unwrap_or_default(),
            album: meta.album.as_deref().unwrap_or_default(),
            genre: meta.genres.line().unwrap_or_default(),
        }
        .pack();
        Self { sort_key: to_natural_sort_key(sort_source), search_text }
    }
}

/// Inserts a track and returns its id. `now` is computed once per batch, so a batch shares one
/// `date_added`.
pub(crate) async fn insert_track(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    file_name: &str,
    meta: &melodia_core::entities::scan::ExtractedMetadata,
    ids: &ResolvedIds,
    now: &str,
    names: &mut NameCache,
) -> Result<i64, AppError> {
    let derived = DerivedColumns::of(meta);

    let sql = format!(
        "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) VALUES ({})",
        placeholders(TRACK_INSERT_COLUMN_COUNT)
    );
    let q = sqlx::query(AssertSqlSafe(sql)).bind(file_path).bind(file_name);

    let q = bind_track_columns(q, meta, ids, &derived);

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
    insert_track_joins(tx, track_id, meta, ids, names).await?;
    Ok(track_id)
}

/// One buffered row for [`insert_tracks_batch`].
pub struct NewTrackRow<'a> {
    pub file_path: String,
    pub file_name: String,
    pub meta: &'a ExtractedMetadata,
    pub ids: ResolvedIds,
}

/// Rows per multi-row INSERT inside the bind budget, the [`LITERAL_DEFAULTS`] riding as SQL.
pub const INSERT_CHUNK_ROWS: usize =
    MAX_BINDS_PER_STATEMENT / (TRACK_INSERT_COLUMN_COUNT - LITERAL_DEFAULTS);

/// [`insert_track`] for the ingest hot path, one statement per [`INSERT_CHUNK_ROWS`] rows. Returns
/// the ids **in `rows` order**, which the drag-and-drop import queues by: `RETURNING`'s order is
/// unspecified, so each id is matched back by its `file_path`.
pub(crate) async fn insert_tracks_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    rows: &[NewTrackRow<'_>],
    now: &str,
    names: &mut NameCache,
) -> Result<Vec<i64>, AppError> {
    let mut out = Vec::with_capacity(rows.len());
    for chunk in rows.chunks(INSERT_CHUNK_ROWS) {
        let derived: Vec<DerivedColumns> =
            chunk.iter().map(|r| DerivedColumns::of(r.meta)).collect();

        let mut qb = sqlx::QueryBuilder::<sqlx::Sqlite>::new(format!(
            "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) "
        ));
        qb.push_values(chunk.iter().zip(derived.iter()), |mut b, (row, derived)| {
            let meta = row.meta;
            b.push_bind(&row.file_path).push_bind(&row.file_name);
            bind_track_columns(&mut b, meta, &row.ids, derived);
            // The `LITERAL_DEFAULTS` in column order, the rating and `date_added` bound among them.
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
                // Every VALUES row returns one; failing beats ids out of step with the rows.
                return Err(AppError::scanner_msg(format!(
                    "multi-row insert returned no id for {}",
                    row.file_path
                )));
            };
            insert_track_joins(tx, id, row.meta, &row.ids, names).await?;
            out.push(id);
        }
    }
    Ok(out)
}

/// Re-points a moved or renamed track, keeping its playback state. `false` where the row is gone,
/// which a caller holding an id resolved before the transaction opened takes as a cue to insert.
pub(crate) async fn update_track_location(
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

/// Rewrites every tag-backed column of a track that changed on disk, keeping its playback state.
///
/// The rating is tag-backed too, but a file that can't carry one (a filename-derived Matroska row,
/// read-only media, the write-back turned off) leaves the row its only copy, hence the `CASE`: the
/// tag wins whenever it says something, and silence leaves the row alone. So with the write-back
/// off, a star set here gives way to one another player wrote, and a cleared one comes back while
/// the file still carries it, the column having no way to say "the user meant zero".
pub(crate) async fn update_track_metadata(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
    meta: &melodia_core::entities::scan::ExtractedMetadata,
    ids: &ResolvedIds,
    names: &mut NameCache,
) -> Result<(), AppError> {
    let derived = DerivedColumns::of(meta);
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
            date_modified = ?, sort_key = ?, search_text = ?,
            rating = CASE WHEN ? = 0 THEN rating ELSE ? END
         WHERE file_path = ? RETURNING id",
    );
    let q = bind_track_columns(q, meta, ids, &derived);
    let tag_rating = meta.rating.unwrap_or(0);
    let updated =
        q.bind(tag_rating).bind(tag_rating).bind(file_path).fetch_optional(&mut **tx).await?;

    // Here rather than at the callers: an artist column refreshed without its credit rows displays
    // one thing and files under another.
    if let Some(row) = updated {
        let track_id: i64 = row.try_get("id")?;
        replace_track_joins(tx, track_id, meta, ids, names).await?;
    }
    Ok(())
}

/// Gives each album missing a cover the artwork of its lowest-id track that has one.
///
/// Driven from the albums, so the cost follows the albums still missing a cover, each one
/// `idx_tracks_album_id` seek in `id` order. Not a window over `tracks`: `SQLite` scans a
/// materialized CTE once per album written, and a first scan writes nearly every album.
pub(crate) async fn update_album_artwork_from_tracks(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    sqlx::query(
        "WITH first_art AS (
            SELECT albums.id AS album_id,
                   (SELECT tracks.artwork_path FROM tracks
                    WHERE tracks.album_id = albums.id
                      AND tracks.artwork_path IS NOT NULL
                      AND tracks.artwork_path != ''
                    ORDER BY tracks.id LIMIT 1) AS artwork_path
            FROM albums
            WHERE albums.artwork_path IS NULL OR albums.artwork_path = ''
        )
        UPDATE albums SET artwork_path = first_art.artwork_path
        FROM first_art
        WHERE first_art.album_id = albums.id
          AND first_art.artwork_path IS NOT NULL",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Fills each album missing a cover from its tracks, then re-points each automatic playlist at
/// its first track's cover.
///
/// One call for a pass that can change which cover a playlisted track holds: refreshing only the
/// albums leaves a playlist naming a cover none of its tracks hold, which keeps the file
/// referenced and out of the artwork sweep's reach.
pub(crate) async fn roll_up_covers(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    update_album_artwork_from_tracks(tx).await?;
    crate::database::queries::playlist::refresh_automatic_thumbnails(tx).await
}

/// Deletes the albums, artists and genres no track reaches any more, which the stats triggers only
/// ever count down.
///
/// Albums first, so an artist whose only album just emptied goes in the same pass, and never the
/// "unknown" artist `albums.artist_id` falls back to. **The credit tables keep an artist alive**,
/// and are all that keeps a featured-only one, so a predicate over the two FKs alone would delete
/// exactly the artists they exist to surface. **Deletes only**: every caller has the stats triggers
/// live or `stats::recalculate_all_stats` behind it, so a recompute here would be done twice.
pub(crate) async fn prune_orphans(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM albums \
         WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.album_id = albums.id)",
    )
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "DELETE FROM artists \
         WHERE id <> ? \
           AND NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM albums WHERE albums.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM track_artists WHERE track_artists.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM album_artists WHERE album_artists.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM track_credits WHERE track_credits.artist_id = artists.id)",
    )
    .bind(crate::database::queries::artist::UNKNOWN_ARTIST_ID)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "DELETE FROM genres \
         WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.genre_id = genres.id) \
           AND NOT EXISTS (SELECT 1 FROM track_genres WHERE track_genres.genre_id = genres.id)",
    )
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub(crate) async fn delete_track_by_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_path: &str,
) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM tracks WHERE file_path = ?")
        .bind(file_path)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes the tracks at `file_paths`, returning how many went.
pub(crate) async fn delete_tracks_by_paths_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_paths: &[String],
) -> Result<u64, AppError> {
    if file_paths.is_empty() {
        return Ok(0);
    }

    let mut total_deleted: u64 = 0;
    for chunk in file_paths.chunks(crate::database::MAX_BINDS_PER_STATEMENT) {
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
