//! The upserts a track resolves its foreign keys through, and the join rows that credit it. An
//! empty name resolves to the sentinel or `None`, so callers stay branch-free.

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

use super::name_cache::NameCache;
use crate::database::queries::search_name::{self, Named};

// One statement per table, each carrying its own key column, so a table can't be paired with the
// wrong one.
const CLEAR_TRACK_ARTISTS: &str = "DELETE FROM track_artists WHERE track_id = ?";
const CLEAR_TRACK_CREDITS: &str = "DELETE FROM track_credits WHERE track_id = ?";
const CLEAR_TRACK_GENRES: &str = "DELETE FROM track_genres WHERE track_id = ?";
const CLEAR_ALBUM_ARTISTS: &str = "DELETE FROM album_artists WHERE album_id = ?";
const INSERT_TRACK_ARTIST: &str =
    "INSERT INTO track_artists (track_id, artist_id, position, join_phrase) VALUES (?, ?, ?, ?)";
const INSERT_ALBUM_ARTIST: &str =
    "INSERT INTO album_artists (album_id, artist_id, position, join_phrase) VALUES (?, ?, ?, ?)";

/// The credit an album files under: its own tag where it has one, else the track artist's
/// **first** name. A guest on one track is no album artist, so "X feat. Y" here would name the
/// album after whichever track reached the upsert first and list it in Y's discography.
#[must_use]
pub fn album_credit_for(meta: &ExtractedMetadata) -> ArtistCredit {
    if meta.album_artist.is_empty() {
        ArtistCredit::from_name(meta.artist.primary_name())
    } else {
        meta.album_artist.clone()
    }
}

/// The name behind [`album_credit_for`], for the caller that needs the grouping key on its own.
#[must_use]
pub fn album_artist_name_for(meta: &ExtractedMetadata) -> &str {
    match meta.album_artist.primary_name() {
        "" => meta.artist.primary_name(),
        name => name,
    }
}

/// The identity tags a credit's names carry beside their spelling, following the opposite rules
/// [`write_credits`] argues.
#[derive(Default, Clone, Copy)]
pub struct CreditDetails<'a> {
    /// `ARTISTSORT` / `ALBUMARTISTSORT`, which describes the whole printed credit rather than any
    /// one name in it.
    pub sort_name: Option<&'a str>,
    /// One `MusicBrainz` id per credited name, in the same order, or empty.
    pub mbids: &'a [String],
}

/// Writes a freshly inserted track's artist credit.
///
/// **Position 0 is the row's own `tracks.artist_id`**, which album grouping and the sort indexes
/// read, so an artist-scoped query and an album-scoped one agree about the track. A file with no
/// artist tag resolves to the sentinel and is still credited to someone.
pub(crate) async fn insert_track_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
    details: CreditDetails<'_>,
    names: &mut NameCache,
) -> Result<(), AppError> {
    write_credits(tx, INSERT_TRACK_ARTIST, track_id, credit, primary_artist_id, details, names)
        .await
}

/// Every join row a freshly inserted track owes: its artist credit, its role credits, its genres.
/// One entry point, since a path writing two of the three leaves a track displaying credits it
/// can't be found by.
pub(crate) async fn insert_track_joins(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    meta: &ExtractedMetadata,
    ids: &super::ResolvedIds,
    names: &mut NameCache,
) -> Result<(), AppError> {
    let details = track_credit_details(meta);
    insert_track_credits(tx, track_id, &meta.artist, ids.artist_id, details, names).await?;
    write_role_credits(tx, track_id, meta, ids.artist_id, names).await?;
    write_genres(tx, track_id, meta, ids.genre_id, names).await
}

/// [`insert_track_joins`] for a track that may already carry rows: a re-ingest or a tag edit.
pub(crate) async fn replace_track_joins(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    meta: &ExtractedMetadata,
    ids: &super::ResolvedIds,
    names: &mut NameCache,
) -> Result<(), AppError> {
    clear_credits(tx, CLEAR_TRACK_ARTISTS, track_id).await?;
    clear_credits(tx, CLEAR_TRACK_CREDITS, track_id).await?;
    clear_credits(tx, CLEAR_TRACK_GENRES, track_id).await?;
    insert_track_joins(tx, track_id, meta, ids, names).await
}

/// The identity tags behind a track's own artist credit.
#[must_use]
pub fn track_credit_details(meta: &ExtractedMetadata) -> CreditDetails<'_> {
    CreditDetails { sort_name: meta.sort.artist.as_deref(), mbids: &meta.artist_mbids }
}

/// The identity tags behind the credit an album files under, falling back as [`album_credit_for`]
/// does so the sort name describes the same credit.
#[must_use]
pub fn album_credit_details(meta: &ExtractedMetadata) -> CreditDetails<'_> {
    if meta.album_artist.is_empty() {
        return track_credit_details(meta);
    }
    CreditDetails { sort_name: meta.sort.album_artist.as_deref(), mbids: &meta.album_artist_mbids }
}

/// One row per role credit, upserting the names it hasn't seen.
///
/// **No stats**, unlike `track_artists`: `artists.track_count` means "credited as an artist", and a
/// composer inflating it would seat them in the Artists grid as a performer. The row keeps the
/// artist alive through `prune_orphans` and nothing else.
async fn write_role_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    meta: &ExtractedMetadata,
    unknown_artist_id: i64,
    names: &mut NameCache,
) -> Result<(), AppError> {
    for (position, credit) in meta.credits.all().iter().enumerate() {
        let artist_id = names.artist(tx, &credit.name, unknown_artist_id).await?;
        sqlx::query(
            "INSERT INTO track_credits (track_id, artist_id, role, detail, position)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(track_id)
        .bind(artist_id)
        .bind(credit.role.as_db_str())
        .bind(&credit.detail)
        .bind(i64::try_from(position).unwrap_or(i64::MAX))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// One row per genre, in tag order.
///
/// Position 0 takes the already-resolved `tracks.genre_id` for [`write_credits`]' reason: the
/// caller resolved that id from this list's first name.
async fn write_genres(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    meta: &ExtractedMetadata,
    primary_genre_id: Option<i64>,
    names: &mut NameCache,
) -> Result<(), AppError> {
    for (position, name) in meta.genres.names().iter().enumerate() {
        let resolved = if position == 0 { primary_genre_id } else { names.genre(tx, name).await? };
        let Some(genre_id) = resolved else {
            continue;
        };
        sqlx::query("INSERT INTO track_genres (track_id, genre_id, position) VALUES (?, ?, ?)")
            .bind(track_id)
            .bind(genre_id)
            .bind(i64::try_from(position).unwrap_or(i64::MAX))
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// [`replace_track_joins`] for an album, plus the rendered credit `album_stats` displays and the
/// `search_name` the Albums grid filters it by. Both of `upsert_album`'s conflict arms reach it,
/// which is why there is no insert-only sibling and why it is that `search_name`'s one writer.
pub(crate) async fn replace_album_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    album_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
    details: CreditDetails<'_>,
    names: &mut NameCache,
) -> Result<(), AppError> {
    clear_credits(tx, CLEAR_ALBUM_ARTISTS, album_id).await?;
    write_credits(tx, INSERT_ALBUM_ARTIST, album_id, credit, primary_artist_id, details, names)
        .await?;

    // NULL rather than the one name it would repeat, so `album_stats` falls back to the artist row
    // and a single-artist album keeps rendering from one place.
    let rendered = (credit.artists().len() > 1).then(|| credit.line().unwrap_or_default());
    sqlx::query("UPDATE albums SET artist_credit = ? WHERE id = ?")
        .bind(rendered)
        .bind(album_id)
        .execute(&mut **tx)
        .await?;
    search_name::refill(tx, Named::Albums, album_id).await
}

/// Drops a parent's whole credit ahead of the replacement: a diff would reconcile positions anyway,
/// and could leave the stats triggers out of step.
async fn clear_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    delete: &'static str,
    parent_id: i64,
) -> Result<(), AppError> {
    sqlx::query(delete).bind(parent_id).execute(&mut **tx).await?;
    Ok(())
}

/// One row per credited name, in order, upserting the names it hasn't seen.
///
/// Position 0 takes `primary_artist_id`, which every caller resolved from this credit's first name.
/// A `MusicBrainz` id is written **per name**, the tag listing one per artist in order; a sort name
/// only for a **solo** credit, `ARTISTSORT` sorting the whole line, so "Beatles, The; Ono, Yoko"
/// stamped on the first artist would file the Beatles under two people. The per-artist
/// `ARTISTSSORT` has no lofty key.
async fn write_credits(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    insert: &'static str,
    parent_id: i64,
    credit: &ArtistCredit,
    primary_artist_id: i64,
    details: CreditDetails<'_>,
    names: &mut NameCache,
) -> Result<(), AppError> {
    if credit.is_empty() {
        sqlx::query(insert)
            .bind(parent_id)
            .bind(primary_artist_id)
            .bind(0_i64)
            .bind("")
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }

    let solo = credit.artists().len() == 1;
    let mut written: Vec<i64> = Vec::with_capacity(credit.artists().len());
    for (position, credited) in credit.artists().iter().enumerate() {
        let artist_id = if position == 0 {
            primary_artist_id
        } else {
            names.artist(tx, &credited.name, primary_artist_id).await?
        };
        let sort_name = if solo { details.sort_name } else { None };
        apply_artist_details(tx, artist_id, sort_name, details.mbids.get(position)).await?;

        // One row per *artist*: two spellings of one name resolve to one `NOCASE` row, which the
        // stats triggers would otherwise count twice. The line keeps both, as the release printed.
        if written.contains(&artist_id) {
            continue;
        }
        written.push(artist_id);

        sqlx::query(insert)
            .bind(parent_id)
            .bind(artist_id)
            .bind(i64::try_from(position).unwrap_or(i64::MAX))
            .bind(&credited.join_phrase)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Fills an artist's sort name and `MusicBrainz` id from the file that named them, **only where
/// still empty**, unlike [`upsert_album`]'s newest-wins release columns. These arrive per credited
/// artist per track, so one disagreeing file would flip the filing order on every rescan; the cost
/// is that a corrected `ARTISTSORT` can't reach a row that has one.
///
/// The emptiness test is spelled twice: the `CASE` arms decide the value, and the `WHERE` keeps
/// `SQLite` from rewriting every matched row whether a value moved or not.
async fn apply_artist_details(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    artist_id: i64,
    sort_name: Option<&str>,
    musicbrainz_id: Option<&String>,
) -> Result<(), AppError> {
    if sort_name.is_none() && musicbrainz_id.is_none() {
        return Ok(());
    }
    let written = sqlx::query(
        "UPDATE artists SET
            sort_name = CASE
                WHEN ? IS NOT NULL AND (sort_name IS NULL OR sort_name = '') THEN ?
                ELSE sort_name END,
            musicbrainz_id = CASE
                WHEN ? IS NOT NULL AND (musicbrainz_id IS NULL OR musicbrainz_id = '')
                THEN ? ELSE musicbrainz_id END
         WHERE id = ?
           AND ((? IS NOT NULL AND (sort_name IS NULL OR sort_name = ''))
             OR (? IS NOT NULL AND (musicbrainz_id IS NULL OR musicbrainz_id = '')))",
    )
    .bind(sort_name)
    .bind(sort_name)
    .bind(musicbrainz_id)
    .bind(musicbrainz_id)
    .bind(artist_id)
    .bind(sort_name)
    .bind(musicbrainz_id)
    .execute(&mut **tx)
    .await?;
    // Either column may have moved; a refill for the id alone is as rare as this write.
    if written.rows_affected() > 0 {
        search_name::refill(tx, Named::Artists, artist_id).await?;
    }
    Ok(())
}

/// Finds or creates an artist by name, `unknown_artist_id` standing in for an empty one. Only a new
/// row writes its `search_name`: the conflict arm re-cases the name, which folds the same.
pub(crate) async fn upsert_artist(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    unknown_artist_id: i64,
) -> Result<i64, AppError> {
    if name.is_empty() {
        return Ok(unknown_artist_id);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO artists (name, search_name) VALUES (?, ?)
         ON CONFLICT(name) DO UPDATE SET name = excluded.name
         RETURNING id",
    )
    .bind(name)
    .bind(search_name::pack(&[name]))
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Finds or creates an album by name and grouping artist, `None` for an empty name. Writes its
/// credit rows too, for [`insert_track_joins`]' reason: an album without them files under nobody.
pub(crate) async fn upsert_album(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    artist_id: i64,
    credit: &ArtistCredit,
    meta: &ExtractedMetadata,
    names: &mut NameCache,
) -> Result<Option<i64>, AppError> {
    if name.is_empty() {
        return Ok(None);
    }
    let release = &meta.release;
    let id = sqlx::query_scalar::<_, i64>(
        // Each release field coalesces, so a track carrying one wins and a later one missing it
        // doesn't blank it, and so nothing here can clear one: `album::clear_release_tags` does
        // after an Edit-Tags save. `is_compilation` is NOT NULL and ORs, one flagged track making
        // the release one, and the same pass unsets it.
        "INSERT INTO albums (
             name, artist_id, year, sort_name, is_compilation,
             musicbrainz_id, musicbrainz_release_group_id,
             label, catalog_number, barcode, media, release_type, release_country
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(name, artist_id) DO UPDATE SET
             name = excluded.name,
             year = COALESCE(excluded.year, albums.year),
             sort_name = COALESCE(excluded.sort_name, albums.sort_name),
             is_compilation = albums.is_compilation OR excluded.is_compilation,
             musicbrainz_id = COALESCE(excluded.musicbrainz_id, albums.musicbrainz_id),
             musicbrainz_release_group_id = COALESCE(
                 excluded.musicbrainz_release_group_id, albums.musicbrainz_release_group_id),
             label = COALESCE(excluded.label, albums.label),
             catalog_number = COALESCE(excluded.catalog_number, albums.catalog_number),
             barcode = COALESCE(excluded.barcode, albums.barcode),
             media = COALESCE(excluded.media, albums.media),
             release_type = COALESCE(excluded.release_type, albums.release_type),
             release_country = COALESCE(excluded.release_country, albums.release_country)
         RETURNING id",
    )
    .bind(name)
    .bind(artist_id)
    .bind(meta.year)
    .bind(meta.sort.album.as_deref())
    .bind(release.is_compilation)
    .bind(&meta.musicbrainz_release_id)
    .bind(&release.musicbrainz_release_group_id)
    .bind(&release.label)
    .bind(&release.catalog_number)
    .bind(&release.barcode)
    .bind(&release.media)
    .bind(&release.release_type)
    .bind(&release.release_country)
    .fetch_one(&mut **tx)
    .await?;
    replace_album_credits(tx, id, credit, artist_id, album_credit_details(meta), names).await?;
    Ok(Some(id))
}

/// Finds or creates a genre by name, `None` for an empty one. Only a new row writes its
/// `search_name`, for [`upsert_artist`]'s reason.
pub(crate) async fn upsert_genre(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<Option<i64>, AppError> {
    if name.is_empty() {
        return Ok(None);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO genres (name, search_name) VALUES (?, ?)
         ON CONFLICT(name) DO UPDATE SET name = excluded.name
         RETURNING id",
    )
    .bind(name)
    .bind(search_name::pack(&[name]))
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(id))
}
