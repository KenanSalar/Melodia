//! A track's people and genres read as rows, for the editor and the lyrics lookup.

use std::collections::HashMap;

use crate::database::{DbPool, chunked_in_query};
use melodia_core::entities::artist::{ArtistCredit, CreditedArtist};
use melodia_core::entities::credits::{CreditRole, RoleCredit, RoleCredits};
use melodia_core::entities::genre::GenreList;
use melodia_core::error::AppError;

/// Each track's ordered artist credit, plus the credit its album carries.
///
/// Two queries rather than one join: the two credits hang off different parents, so a single
/// statement would multiply each track's rows by its album's. The `ORDER BY` is what makes the
/// credit an *ordered* thing rather than a set.
///
/// Used by the Edit-Tags dialog over a multi-track selection. A single track reads its credit off
/// the file instead, that being the authority and already open for the lyrics tab.
pub async fn get_track_credits_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<HashMap<i64, (ArtistCredit, ArtistCredit)>, AppError> {
    let track_rows: Vec<(i64, String, String)> = chunked_in_query(db.read(), ids, |placeholders| {
        format!(
            "SELECT ta.track_id, a.name, ta.join_phrase \
                 FROM track_artists ta JOIN artists a ON a.id = ta.artist_id \
                 WHERE ta.track_id IN ({placeholders}) ORDER BY ta.track_id, ta.position"
        )
    })
    .await?;

    let album_rows: Vec<(i64, String, String)> = chunked_in_query(db.read(), ids, |placeholders| {
        format!(
            "SELECT t.id, a.name, aa.join_phrase \
                 FROM tracks t \
                 JOIN album_artists aa ON aa.album_id = t.album_id \
                 JOIN artists a ON a.id = aa.artist_id \
                 WHERE t.id IN ({placeholders}) ORDER BY t.id, aa.position"
        )
    })
    .await?;

    let mut out: HashMap<i64, (ArtistCredit, ArtistCredit)> = HashMap::with_capacity(ids.len());
    for (id, artists) in group_credits(track_rows) {
        out.entry(id).or_default().0 = ArtistCredit::new(artists);
    }
    for (id, artists) in group_credits(album_rows) {
        out.entry(id).or_default().1 = ArtistCredit::new(artists);
    }
    Ok(out)
}

/// Every track's role credits, keyed by track id.
///
/// A sibling of [`get_track_credits_by_ids`] rather than a third query inside it: the Edit-Tags
/// dialog is the only caller that needs both, and a track with no role credits is the common case
/// — most files name nobody but their artist, so the join returns nothing for them and they get a
/// `RoleCredits::default()` from the `or_default` rather than a row.
///
/// A role this build doesn't know is skipped: the column is a string, so a database written by a
/// newer version can hold one, and dropping it beats inventing a variant for it.
pub async fn get_track_role_credits_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<HashMap<i64, RoleCredits>, AppError> {
    let rows: Vec<(i64, String, String, String)> =
        chunked_in_query(db.read(), ids, |placeholders| {
            format!(
                "SELECT tc.track_id, a.name, tc.role, tc.detail \
                 FROM track_credits tc JOIN artists a ON a.id = tc.artist_id \
                 WHERE tc.track_id IN ({placeholders}) ORDER BY tc.track_id, tc.position"
            )
        })
        .await?;

    let mut grouped: HashMap<i64, Vec<RoleCredit>> = HashMap::new();
    for (id, name, role, detail) in rows {
        let Some(role) = CreditRole::from_db_str(&role) else {
            continue;
        };
        grouped.entry(id).or_default().push(RoleCredit { role, name, detail });
    }
    Ok(grouped.into_iter().map(|(id, credits)| (id, RoleCredits::new(credits))).collect())
}

/// Every track's genres, keyed by track id.
///
/// Read as rows rather than by splitting the rendered `tracks.genre` column, which is what the
/// Edit-Tags dialog did while its genre field was one box: a genre containing the separator
/// (`Chanson, Francaise`) came back as two, and saving made that permanent. The rows are the truth
/// and the column is derived from them, so the editor reads the truth.
pub async fn get_track_genres_by_ids(
    db: &DbPool,
    ids: &[i64],
) -> Result<HashMap<i64, GenreList>, AppError> {
    let rows: Vec<(i64, String)> = chunked_in_query(db.read(), ids, |placeholders| {
        format!(
            "SELECT tg.track_id, g.name \
             FROM track_genres tg JOIN genres g ON g.id = tg.genre_id \
             WHERE tg.track_id IN ({placeholders}) ORDER BY tg.track_id, tg.position"
        )
    })
    .await?;

    let mut grouped: HashMap<i64, Vec<String>> = HashMap::new();
    for (id, name) in rows {
        grouped.entry(id).or_default().push(name);
    }
    Ok(grouped.into_iter().map(|(id, names)| (id, GenreList::new(names))).collect())
}

/// One track's ordered artist credit, the album's left alone.
///
/// [`get_track_credits_by_ids`] over a single parent minus its second query, for the lyrics
/// lookup: it asks about the playing track and has no album credit to show, and the caller is
/// about to open a socket, so the row multiplication that one avoids is not a cost worth paying
/// here either.
pub async fn get_track_credit(db: &DbPool, id: i64) -> Result<ArtistCredit, AppError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT a.name, ta.join_phrase \
         FROM track_artists ta JOIN artists a ON a.id = ta.artist_id \
         WHERE ta.track_id = ? ORDER BY ta.position",
    )
    .bind(id)
    .fetch_all(db.read())
    .await?;

    Ok(ArtistCredit::new(
        rows.into_iter().map(|(name, join_phrase)| CreditedArtist { name, join_phrase }).collect(),
    ))
}

/// Collapse `(parent id, name, join phrase)` rows into one credit per parent, keeping the order
/// the query returned them in.
fn group_credits(rows: Vec<(i64, String, String)>) -> HashMap<i64, Vec<CreditedArtist>> {
    let mut grouped: HashMap<i64, Vec<CreditedArtist>> = HashMap::new();
    for (id, name, join_phrase) in rows {
        grouped.entry(id).or_default().push(CreditedArtist { name, join_phrase });
    }
    grouped
}
