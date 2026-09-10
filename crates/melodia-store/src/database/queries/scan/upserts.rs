//! FK helpers: upsert an `artist` / `album` / `genre` row by name and
//! return its rowid. Each returns the supplied "unknown" sentinel id (or
//! `None`) for empty names so callers can stay branch-free.

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

use super::name_cache::NameCache;

/// The statements a credit rewrite is made of, one per table.
///
/// Literals rather than a `format!` over a table name, which cost a `String` per re-ingested
/// track to say the same thing every time. Each carries its own key column, so there is no way
/// to pair a table with the wrong one.
const CLEAR_TRACK_ARTISTS: &str = "DELETE FROM track_artists WHERE track_id = ?";
const CLEAR_TRACK_CREDITS: &str = "DELETE FROM track_credits WHERE track_id = ?";
const CLEAR_TRACK_GENRES: &str = "DELETE FROM track_genres WHERE track_id = ?";
const CLEAR_ALBUM_ARTISTS: &str = "DELETE FROM album_artists WHERE album_id = ?";
const INSERT_TRACK_ARTIST: &str =
    "INSERT INTO track_artists (track_id, artist_id, position, join_phrase) VALUES (?, ?, ?, ?)";
const INSERT_ALBUM_ARTIST: &str =
    "INSERT INTO album_artists (album_id, artist_id, position, join_phrase) VALUES (?, ?, ?, ?)";

/// The credit an album files under: its own tag where it has one, else the track artist's
/// **first** name.
///
/// The whole track credit is the wrong fallback and the reason is the one the album-artist
/// fallback already exists for. A guest on one track is not an album artist, so taking
/// "X feat. Y" here would rename the album after whichever track happened to reach the upsert
/// first and list the album in Y's discography. Same answer as [`album_artist_name_for`], one
/// shape up.
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

/// The identity tags a credit's names carry beside their spelling.
///
/// Its own type because the two travel to the same place and follow opposite rules — see
/// [`write_credits`] for why one is position-aligned and the other is not.
#[derive(Default, Clone, Copy)]
pub struct CreditDetails<'a> {
    /// `ARTISTSORT` / `ALBUMARTISTSORT`, which describes the whole printed credit rather than any
    /// one name in it.
    pub sort_name: Option<&'a str>,
    /// One `MusicBrainz` id per credited name, in the same order, or empty.
    pub mbids: &'a [String],
}

/// Write a freshly inserted track's artist credit.
///
/// **Position 0 is the row's own `tracks.artist_id`.** Album grouping and the sort indexes still
/// read that FK, so the join table agreeing with it at the head is what stops an artist-scoped
/// query and an album-scoped one disagreeing about the same track. `primary_artist_id` covers the
/// one case the credit can't: a file with no artist tag, which resolves to the sentinel and still
/// gets its row rather than being credited to nobody.
pub async fn insert_track_credits(
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
///
/// One entry point rather than three calls at each of the two insert sites, because the three
/// tables are one fact about the row — a path that wrote two of them is a track that displays
/// credits it cannot be found by, and the bug is invisible from either half.
pub async fn insert_track_joins(
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
pub async fn replace_track_joins(
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
    CreditDetails {
        sort_name: meta.sort.artist.as_deref(),
        mbids: &meta.artist_mbids,
    }
}

/// The identity tags behind the credit an album files under.
///
/// Falls back to the track artist's the way [`album_credit_for`] does, so the sort name and the
/// credit it describes come from the same tag rather than from different ones.
#[must_use]
pub fn album_credit_details(meta: &ExtractedMetadata) -> CreditDetails<'_> {
    if meta.album_artist.is_empty() {
        return track_credit_details(meta);
    }
    CreditDetails {
        sort_name: meta.sort.album_artist.as_deref(),
        mbids: &meta.album_artist_mbids,
    }
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
        let resolved = if position == 0 {
            primary_genre_id
        } else {
            names.genre(tx, name).await?
        };
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

/// [`replace_track_credits`] for an album, plus the rendered credit `album_stats` displays.
///
/// No insert-only sibling: `upsert_album` reaches this down both arms of its `ON CONFLICT`, so the
/// rows may or may not be there and only the delete can tell.
pub async fn replace_album_credits(
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
    Ok(())
}

/// Drop a parent's whole credit, ahead of writing the replacement.
///
/// Delete-then-insert rather than a diff: a credit is a handful of ordered rows, so a diff would
/// have to reconcile positions anyway and buys nothing but a way to leave the stats triggers out
/// of step.
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
/// Position 0 takes `primary_artist_id` rather than an upsert of its own: every caller resolved
/// that id *from* this credit's first name, so the round trip would ask a question it is holding
/// the answer to. Which is also the invariant `album_credit_for` exists to keep true.
///
/// The two halves of `details` follow opposite rules, and both are conventions rather than
/// choices. A `MusicBrainz` id is written **per name**, the tag listing one per credited artist in
/// the same order. A sort name is written **only for a solo credit**: `ARTISTSORT` is the sort form
/// of the whole printed line, so on "The Beatles & Yoko Ono" it reads "Beatles, The; Ono, Yoko" and
/// stamping that onto the first artist files the Beatles under a string naming two people. The
/// per-artist plural (`ARTISTSSORT`) has no lofty key, so a multi-artist credit keeps the locally
/// derived sort name.
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

        // One row per *artist*, not per name. `artists.name` is `UNIQUE COLLATE NOCASE`, so a
        // credit naming someone twice under two spellings resolves to one row here — and the
        // stats triggers on this table would then count the track twice for them. The line keeps
        // both names, being what the release printed; the join rows are what a count reads.
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

/// Fill in an artist's sort name and `MusicBrainz` id from the file that named them.
///
/// **Only where the column is still empty**, deliberately unlike the release columns in
/// [`upsert_album`], which take the newest value. These arrive once per *credited artist per
/// track* rather than once per release, so a library where one file disagrees with its neighbours
/// would flip the artist's filing order on every rescan depending on which file reached the upsert
/// last.
///
/// The cost is that first writer wins for good: no surface edits these, so a corrected
/// `ARTISTSORT` in the file cannot reach a row that already has one.
///
/// The emptiness test is spelled twice on purpose. The `CASE` arms decide the value but still
/// match the row, and `SQLite` rewrites a matched row whether or not a value moved. On a tagged
/// library that is one write per credited artist per track, for nothing.
async fn apply_artist_details(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    artist_id: i64,
    sort_name: Option<&str>,
    musicbrainz_id: Option<&String>,
) -> Result<(), AppError> {
    if sort_name.is_none() && musicbrainz_id.is_none() {
        return Ok(());
    }
    sqlx::query(
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
    Ok(())
}

/// Find or create an artist by name, returning the artist ID.
/// Uses the provided `unknown_artist_id` for empty artist names.
pub async fn upsert_artist(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    unknown_artist_id: i64,
) -> Result<i64, AppError> {
    if name.is_empty() {
        return Ok(unknown_artist_id);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO artists (name) VALUES (?)
         ON CONFLICT(name) DO UPDATE SET name = excluded.name
         RETURNING id",
    )
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Find or create an album by name and artist, returning the album ID.
/// Returns None if the album name is empty.
///
/// `artist_id` is the album's grouping key, the primary name behind `credit`, and the two are
/// separate arguments because the caller has already resolved and cached the id. The credit rows
/// are written here rather than by the caller for the reason [`replace_track_credits`] gives: an
/// album row without them files under nobody.
pub async fn upsert_album(
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
        // Every release field is `COALESCE(excluded.x, albums.x)`: it updates the stored value on
        // re-ingest (e.g. a tag edit) but preserves it when the new value is NULL, so a track
        // carrying one wins and a later track missing it doesn't blank it. Which is also why
        // nothing here can *clear* one: a cleared field arrives as the NULL this coalesces away,
        // so the Edit-Tags path nulls it explicitly afterwards (`library::tags::clear_release_tags`).
        //
        // `is_compilation` is the one exception, and an OR rather than a COALESCE: the column is
        // NOT NULL so there is no "said nothing" to coalesce against, and one track flagged makes
        // the release one. It is unset by that same pass.
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

/// Find or create a genre by name, returning the genre ID.
/// Returns None if the genre name is empty.
pub async fn upsert_genre(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<Option<i64>, AppError> {
    if name.is_empty() {
        return Ok(None);
    }
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO genres (name) VALUES (?)
         ON CONFLICT(name) DO UPDATE SET name = excluded.name
         RETURNING id",
    )
    .bind(name)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some(id))
}
