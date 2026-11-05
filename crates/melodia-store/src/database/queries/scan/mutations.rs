//! The track rows' write side, and the cover roll-ups and orphan prune at the tail of a batch.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::database::sql::Sql;
use crate::database::{MAX_BINDS_PER_STATEMENT, chunked_in_execute_on, placeholders, values_list};
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

/// `play_count` through `date_added` as a new row starts them, the file's rating and `date_added`
/// bound among the literals.
const PLAYBACK_TAIL: &str = "0, 0, ?, 0, NULL, 0, ?";

/// The literals in [`PLAYBACK_TAIL`], which spend no bind.
const LITERAL_DEFAULTS: usize = 5;

/// One `VALUES` tuple of [`TRACK_INSERT_COLUMNS`], as [`bind_new_track`] binds it.
static TRACK_ROW: LazyLock<String> = LazyLock::new(|| {
    let bound = TRACK_INSERT_COLUMN_COUNT - column_count(PLAYBACK_TAIL);
    format!("({}, {PLAYBACK_TAIL})", placeholders(bound))
});

/// Binds one [`TRACK_ROW`].
fn bind_new_track(sql: &mut Sql, row: &NewTrackRow<'_>, now: &str) {
    sql.bind(row.file_path.as_str()).bind(row.file_name.as_str());
    bind_track_columns(sql, row.meta, &row.ids);
    sql.bind(row.meta.rating.unwrap_or(0)).bind(now);
}

/// Binds the block every track write shares: the metadata from `file_hash` to `artwork_path`, the
/// four foreign keys, `date_modified` and the two [`DerivedColumns`]. The one place their order is
/// written, where a second copy could swap two same-typed nullable columns silently.
fn bind_track_columns(sql: &mut Sql, meta: &ExtractedMetadata, ids: &ResolvedIds) {
    let derived = DerivedColumns::of(meta);
    sql.bind(meta.file_hash.as_str())
        .bind(meta.title.as_str())
        .bind(meta.artist.line())
        .bind(meta.album_artist.line())
        .bind(meta.album.as_deref())
        .bind(meta.genres.line())
        .bind(meta.credits.line())
        .bind(meta.track_number)
        .bind(meta.track_total)
        .bind(meta.disc_number)
        .bind(meta.disc_total)
        .bind(meta.disc_subtitle.as_deref())
        .bind(meta.subtitle.as_deref())
        .bind(meta.release_date.as_deref())
        .bind(meta.year)
        .bind(meta.original_date.as_deref())
        .bind(meta.original_year)
        .bind(meta.comment.as_deref())
        .bind(meta.bpm)
        .bind(meta.initial_key.as_deref())
        .bind(meta.mood.as_deref())
        .bind(meta.grouping.as_deref())
        .bind(meta.work.as_deref())
        .bind(meta.movement.as_deref())
        .bind(meta.movement_number)
        .bind(meta.movement_total)
        .bind(meta.language.as_deref())
        .bind(meta.copyright.as_deref())
        .bind(meta.isrc.as_deref())
        .bind(meta.musicbrainz_track_id.as_deref())
        .bind(meta.musicbrainz_release_id.as_deref())
        .bind(meta.musicbrainz_release_track_id.as_deref())
        .bind(meta.replaygain_track_gain)
        .bind(meta.replaygain_track_peak)
        .bind(meta.replaygain_album_gain)
        .bind(meta.replaygain_album_peak)
        .bind(meta.duration_ms)
        .bind(meta.file_size)
        .bind(meta.codec.as_deref())
        .bind(meta.bitrate)
        .bind(meta.channels)
        .bind(meta.sample_rate)
        .bind(meta.bit_depth)
        .bind(meta.artwork_path.as_deref())
        .bind(ids.album_id)
        .bind(ids.artist_id)
        .bind(ids.genre_id)
        .bind(ids.folder_id)
        .bind(meta.date_modified.as_deref())
        .bind(derived.sort_key)
        .bind(derived.search_text);
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

static INSERT_TRACK: LazyLock<String> = LazyLock::new(|| {
    format!(
        "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) VALUES {} RETURNING id",
        TRACK_ROW.as_str()
    )
});

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
    let row = NewTrackRow {
        file_path: file_path.to_owned(),
        file_name: file_name.to_owned(),
        meta,
        ids: *ids,
    };
    let mut sql = Sql::new(INSERT_TRACK.as_str());
    bind_new_track(&mut sql, &row, now);
    let (track_id,): (i64,) = sql.fetch_one(&mut **tx).await?;
    insert_track_joins(tx, track_id, meta, ids, names).await?;
    Ok(track_id)
}

/// One new track's row, for [`insert_track`] or [`insert_tracks_batch`].
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
        let mut sql = Sql::trusted(format!(
            "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) VALUES {} RETURNING id, file_path",
            values_list(&TRACK_ROW, chunk.len())
        ));
        for row in chunk {
            bind_new_track(&mut sql, row, now);
        }

        let returned: Vec<(i64, String)> = sql.fetch_all(&mut **tx).await?;
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
    let mut sql = Sql::new(
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
    bind_track_columns(&mut sql, meta, ids);
    let tag_rating = meta.rating.unwrap_or(0);
    sql.bind(tag_rating).bind(tag_rating).bind(file_path);
    let updated: Option<(i64,)> = sql.fetch_optional(&mut **tx).await?;

    // Here rather than at the callers: an artist column refreshed without its credit rows displays
    // one thing and files under another.
    if let Some((track_id,)) = updated {
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
    chunked_in_execute_on(tx, file_paths, |placeholders| {
        format!("DELETE FROM tracks WHERE file_path IN ({placeholders})")
    })
    .await
}
