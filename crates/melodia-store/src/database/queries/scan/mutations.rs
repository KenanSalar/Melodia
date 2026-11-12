//! The track rows' write side, and the cover roll-ups and orphan prune at the tail of a batch.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use rusqlite::Transaction;

use crate::database::queries::track::file_rows::WHOLE_FILE;
use crate::database::queries::track::order_keys::OrderKeys;
use crate::database::sql::{self, Sql};
use crate::database::{MAX_BINDS_PER_STATEMENT, chunked_in_execute, placeholders, values_list};
use melodia_core::entities::cue::{CueSpan, CueTrack};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::track_search::SearchFields;
use melodia_core::error::AppError;

use super::ResolvedIds;
use super::name_cache::NameCache;
use super::sort_key::to_natural_sort_key;
use super::upserts::{insert_track_joins, replace_track_joins};

/// The columns [`insert_track`] and [`insert_tracks_batch`] write, in the order
/// [`bind_track_columns`] binds them, which [`TAG_BACKED_SET`] follows too. The placeholder grid
/// and the bind budget are counted off this string.
const TRACK_INSERT_COLUMNS: &str = "file_path, file_name,
    cue_track, cue_start, cue_end, cue_sheet_id,
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
    date_modified, sort_key, search_text, artist_key, album_key, genre_key,
    play_count, skip_count, rating, is_favorite, last_played, last_position,
    date_added";

/// How many times `byte` occurs in `text`, at compile time.
const fn count_byte(text: &str, byte: u8) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == byte {
            count += 1;
        }
        i += 1;
    }
    count
}

/// How many names a comma-separated column list holds, at compile time.
const fn column_count(list: &str) -> usize {
    count_byte(list, b',') + 1
}

const TRACK_INSERT_COLUMN_COUNT: usize = column_count(TRACK_INSERT_COLUMNS);

/// `play_count` through `date_added` as a new row starts them, the file's rating and `date_added`
/// bound among the literals.
const PLAYBACK_TAIL: &str = "0, 0, ?, 0, NULL, 0, ?";

/// The literals in [`PLAYBACK_TAIL`], which spend no bind.
const LITERAL_DEFAULTS: usize = column_count(PLAYBACK_TAIL) - count_byte(PLAYBACK_TAIL, b'?');

/// One `VALUES` tuple of [`TRACK_INSERT_COLUMNS`], as [`bind_new_track`] binds it.
static TRACK_ROW: LazyLock<String> = LazyLock::new(|| {
    let bound = TRACK_INSERT_COLUMN_COUNT - column_count(PLAYBACK_TAIL);
    format!("({}, {PLAYBACK_TAIL})", placeholders(bound))
});

/// Binds one [`TRACK_ROW`].
fn bind_new_track(sql: &mut Sql, row: &NewTrackRow<'_>, now: &str) {
    sql.bind(row.file_path.as_str()).bind(row.file_name.as_str());
    let cue = row.cue.as_ref();
    sql.bind(cue.map(|cue| cue.track))
        .bind(cue.map(|cue| cue.span.start()))
        .bind(cue.and_then(|cue| cue.span.end()))
        .bind(cue.and_then(|cue| cue.sheet_id));
    bind_track_columns(sql, row.meta, &row.ids);
    sql.bind(row.meta.rating.unwrap_or(0)).bind(now);
}

/// Binds the block every track write shares: the metadata from `file_hash` to `artwork_path`, the
/// four foreign keys, `date_modified` and the [`DerivedColumns`]. The one place their order is
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
        .bind(ids.album_id())
        .bind(ids.artist_id())
        .bind(ids.genre_id())
        .bind(ids.folder_id)
        .bind(meta.date_modified.as_deref())
        .bind(derived.sort_key)
        .bind(derived.search_text)
        .bind(derived.order.artist)
        .bind(derived.order.album)
        .bind(derived.order.genre);
}

/// The columns computed from a file's tags rather than read off them, one rule for every write: a
/// row sorting or filtering differently from its neighbours is invisible until a list is read top
/// to bottom.
struct DerivedColumns {
    sort_key: String,
    search_text: String,
    order: OrderKeys,
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
        let order = OrderKeys::of(meta.artist.line(), meta.album.as_deref(), meta.genres.line());
        Self { sort_key: to_natural_sort_key(sort_source), search_text, order }
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
pub(crate) fn insert_track(
    tx: &Transaction<'_>,
    file_path: &str,
    file_name: &str,
    meta: &ExtractedMetadata,
    ids: &ResolvedIds,
    now: &str,
    names: &mut NameCache,
) -> Result<i64, AppError> {
    let row = NewTrackRow {
        file_path: file_path.to_owned(),
        file_name: file_name.to_owned(),
        cue: None,
        meta,
        ids: *ids,
    };
    let mut sql = Sql::new(INSERT_TRACK.as_str());
    bind_new_track(&mut sql, &row, now);
    let track_id: i64 = sql.fetch_one(tx)?;
    insert_track_joins(tx, track_id, meta, ids, names)?;
    Ok(track_id)
}

/// What a track row's `file_name` column holds for `path`.
pub(crate) fn file_name_of(path: &Path) -> String {
    path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_owned()
}

/// One new track's row, for [`insert_track`] or [`insert_tracks_batch`].
pub struct NewTrackRow<'a> {
    pub file_path: String,
    pub file_name: String,
    /// `None` for a row playing its whole file.
    pub cue: Option<CuePlacement>,
    pub meta: &'a ExtractedMetadata,
    pub ids: ResolvedIds,
}

/// Where a cue sheet puts a row in its file.
#[derive(Clone, Copy)]
pub struct CuePlacement {
    pub track: CueTrack,
    pub span: CueSpan,
    /// `None` for a sheet embedded in the file.
    pub sheet_id: Option<i64>,
}

/// Rows per multi-row INSERT inside the bind budget, the [`LITERAL_DEFAULTS`] riding as SQL.
pub const INSERT_CHUNK_ROWS: usize =
    MAX_BINDS_PER_STATEMENT / (TRACK_INSERT_COLUMN_COUNT - LITERAL_DEFAULTS);

/// [`insert_track`] for the ingest hot path, one statement per [`INSERT_CHUNK_ROWS`] rows. Returns
/// the ids **in `rows` order**, which the drag-and-drop import queues by: `RETURNING`'s order is
/// unspecified, so each id is matched back by its `file_path` and `cue_track`.
pub(crate) fn insert_tracks_batch(
    tx: &Transaction<'_>,
    rows: &[NewTrackRow<'_>],
    now: &str,
    names: &mut NameCache,
) -> Result<Vec<i64>, AppError> {
    let mut out = Vec::with_capacity(rows.len());
    for chunk in rows.chunks(INSERT_CHUNK_ROWS) {
        let mut sql = Sql::vouched(format!(
            "INSERT INTO tracks ({TRACK_INSERT_COLUMNS}) VALUES {} \
             RETURNING id, file_path, cue_track",
            values_list(&TRACK_ROW, chunk.len())
        ));
        for row in chunk {
            bind_new_track(&mut sql, row, now);
        }

        let returned: Vec<(i64, String, Option<CueTrack>)> = sql.fetch_all(tx)?;
        let mut by_key: HashMap<(&str, Option<CueTrack>), i64> = returned
            .iter()
            .map(|(id, path, cue_track)| ((path.as_str(), *cue_track), *id))
            .collect();
        for row in chunk {
            let key = (row.file_path.as_str(), row.cue.map(|cue| cue.track));
            let Some(id) = by_key.remove(&key) else {
                // Every VALUES row returns one; failing beats ids out of step with the rows.
                return Err(AppError::scanner_msg(format!(
                    "multi-row insert returned no id for {}",
                    row.file_path
                )));
            };
            insert_track_joins(tx, id, row.meta, &row.ids, names)?;
            out.push(id);
        }
    }
    Ok(out)
}

/// Re-points every row of a moved or renamed file, keeping their playback state. `false` where the
/// rows are gone, which a caller holding a path read before the transaction opened takes as a cue
/// to insert.
pub(crate) fn update_track_location(
    tx: &Transaction<'_>,
    old_path: &str,
    new_path: &str,
    new_file_name: &str,
    new_folder_id: i64,
    date_modified: Option<&str>,
) -> Result<bool, AppError> {
    let updated = sql::execute(
        tx,
        "UPDATE tracks SET file_path = ?, file_name = ?, folder_id = ?, date_modified = ?
         WHERE file_path = ?",
        (new_path, new_file_name, new_folder_id, date_modified, old_path),
    )?;
    Ok(updated > 0)
}

/// The `SET` list of every tag-backed column, in [`bind_tag_update`]'s order.
const TAG_BACKED_SET: &str = "
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
    artist_key = ?, album_key = ?, genre_key = ?,
    rating = CASE WHEN ? = 0 THEN rating ELSE ? END";

/// Binds [`TAG_BACKED_SET`].
fn bind_tag_update(sql: &mut Sql, meta: &ExtractedMetadata, ids: &ResolvedIds) {
    bind_track_columns(sql, meta, ids);
    let tag_rating = meta.rating.unwrap_or(0);
    sql.bind(tag_rating).bind(tag_rating);
}

static UPDATE_TRACK_METADATA: LazyLock<String> = LazyLock::new(|| {
    format!("UPDATE tracks SET {TAG_BACKED_SET} WHERE file_path = ? AND {WHOLE_FILE} RETURNING id")
});

static UPDATE_CUE_TRACK: LazyLock<String> = LazyLock::new(|| {
    format!(
        "UPDATE tracks SET {TAG_BACKED_SET}, cue_start = ?, cue_end = ?, cue_sheet_id = ?
         WHERE id = ?"
    )
});

/// Rewrites every tag-backed column of a file's own row when the file changed on disk, keeping its
/// playback state. `false` where the file has no row of its own: gone, or split by a cue sheet.
///
/// The rating is tag-backed too, but a file that can't carry one (a filename-derived Matroska row,
/// read-only media, the write-back turned off) leaves the row its only copy, hence the `CASE`: the
/// tag wins whenever it says something, and silence leaves the row alone. So with the write-back
/// off, a star set here gives way to one another player wrote, and a cleared one comes back while
/// the file still carries it, the column having no way to say "the user meant zero".
pub(crate) fn update_track_metadata(
    tx: &Transaction<'_>,
    file_path: &str,
    meta: &ExtractedMetadata,
    ids: &ResolvedIds,
    names: &mut NameCache,
) -> Result<bool, AppError> {
    let mut sql = Sql::new(UPDATE_TRACK_METADATA.as_str());
    bind_tag_update(&mut sql, meta, ids);
    sql.bind(file_path);
    let Some(track_id) = sql.fetch_optional::<i64>(tx)? else {
        return Ok(false);
    };

    // Here rather than at the callers: an artist column refreshed without its credit rows displays
    // one thing and files under another.
    replace_track_joins(tx, track_id, meta, ids, names)?;
    Ok(true)
}

/// [`update_track_metadata`] for the sheet track `track_id`, which a reread sheet may also have
/// moved or handed to another sheet.
pub(crate) fn update_cue_track(
    tx: &Transaction<'_>,
    track_id: i64,
    cue: CuePlacement,
    meta: &ExtractedMetadata,
    ids: &ResolvedIds,
    names: &mut NameCache,
) -> Result<(), AppError> {
    let mut sql = Sql::new(UPDATE_CUE_TRACK.as_str());
    bind_tag_update(&mut sql, meta, ids);
    sql.bind(cue.span.start()).bind(cue.span.end()).bind(cue.sheet_id).bind(track_id);
    sql.execute(tx)?;
    replace_track_joins(tx, track_id, meta, ids, names)
}

/// Gives each album missing a cover the artwork of its lowest-id track that has one.
///
/// Driven from the albums, so the cost follows the albums still missing a cover, each one
/// `idx_tracks_album_id` seek in `id` order. Not a window over `tracks`: `SQLite` scans a
/// materialized CTE once per album written, and a first scan writes nearly every album.
pub(crate) fn update_album_artwork_from_tracks(tx: &Transaction<'_>) -> Result<(), AppError> {
    sql::execute(
        tx,
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
        (),
    )?;
    Ok(())
}

/// Fills each album missing a cover from its tracks, then re-points each automatic playlist at
/// its first track's cover.
///
/// One call for a pass that can change which cover a playlisted track holds: refreshing only the
/// albums leaves a playlist naming a cover none of its tracks hold, which keeps the file
/// referenced and out of the artwork sweep's reach.
pub(crate) fn roll_up_covers(tx: &Transaction<'_>) -> Result<(), AppError> {
    update_album_artwork_from_tracks(tx)?;
    crate::database::queries::playlist::refresh_automatic_thumbnails(tx)
}

/// Deletes the albums, artists and genres no track reaches any more, which the stats triggers only
/// ever count down.
///
/// Albums first, so an artist whose only album just emptied goes in the same pass, and never the
/// "unknown" artist `albums.artist_id` falls back to. **The credit tables keep an artist alive**,
/// and are all that keeps a featured-only one, so a predicate over the two FKs alone would delete
/// exactly the artists they exist to surface. **Deletes only**: every caller has the stats triggers
/// live or `stats::recalculate_all_stats` behind it, so a recompute here would be done twice.
pub(crate) fn prune_orphans(tx: &Transaction<'_>) -> Result<(), AppError> {
    sql::execute(
        tx,
        "DELETE FROM albums \
         WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.album_id = albums.id)",
        (),
    )?;

    sql::execute(
        tx,
        "DELETE FROM artists \
         WHERE id <> ? \
           AND NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM albums WHERE albums.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM track_artists WHERE track_artists.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM album_artists WHERE album_artists.artist_id = artists.id) \
           AND NOT EXISTS (SELECT 1 FROM track_credits WHERE track_credits.artist_id = artists.id)",
        (crate::database::queries::artist::UNKNOWN_ARTIST_ID,),
    )?;

    sql::execute(
        tx,
        "DELETE FROM genres \
         WHERE NOT EXISTS (SELECT 1 FROM tracks WHERE tracks.genre_id = genres.id) \
           AND NOT EXISTS (SELECT 1 FROM track_genres WHERE track_genres.genre_id = genres.id)",
        (),
    )?;

    Ok(())
}

pub(crate) fn delete_track_by_path(
    tx: &Transaction<'_>,
    file_path: &str,
) -> Result<bool, AppError> {
    let deleted = sql::execute(tx, "DELETE FROM tracks WHERE file_path = ?", (file_path,))?;
    Ok(deleted > 0)
}

/// Deletes the tracks at `file_paths`, returning how many went.
pub(crate) fn delete_tracks_by_paths_batch(
    tx: &Transaction<'_>,
    file_paths: &[String],
) -> Result<u64, AppError> {
    chunked_in_execute(tx, file_paths, |placeholders| {
        format!("DELETE FROM tracks WHERE file_path IN ({placeholders})")
    })
}

/// Deletes the tracks `ids` name, their playlist entries going with them by `ON DELETE CASCADE`.
pub(crate) fn delete_tracks_by_ids(tx: &Transaction<'_>, ids: &[i64]) -> Result<(), AppError> {
    chunked_in_execute(tx, ids, |placeholders| {
        format!("DELETE FROM tracks WHERE id IN ({placeholders})")
    })?;
    Ok(())
}
