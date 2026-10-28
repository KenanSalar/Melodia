//! The tracks behind a set of entity cards, which every card right-click action acts on: the one
//! place that knows how an album, artist, genre or playlist becomes a track list. Browse's folder
//! cards aren't here, a folder being a path rather than an id.

use std::collections::{HashMap, HashSet};

use melodia_core::entities::smart_criteria::SmartCriteria;
use melodia_core::error::{AppError, describe};
use melodia_store::database::{DbPool, queries};

/// Which kind of card a grid's menu is acting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Album,
    Artist,
    Genre,
    Playlist,
    /// A card that already *is* a track: the two Most Played grids and Browse's file cards.
    Track,
}

impl EntityKind {
    /// Parse the token a `.slint` menu mount spells.
    ///
    /// `None` for anything else, so a mistyped mount resolves to no tracks and logs, rather than
    /// falling through to whichever kind happened to be first.
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "album" => Some(Self::Album),
            "artist" => Some(Self::Artist),
            "genre" => Some(Self::Genre),
            "playlist" => Some(Self::Playlist),
            "track" => Some(Self::Track),
            _ => None,
        }
    }
}

/// Every track behind `ids`: the entities in the order they were asked for, each one's tracks in
/// its own order, and no track twice.
///
/// The dedupe is load-bearing: a track credited to two selected artists, or in two selected
/// playlists, is queued once.
pub async fn track_ids_for(
    db: &DbPool,
    kind: EntityKind,
    ids: &[i64],
) -> Result<Vec<i64>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let pairs = match kind {
        EntityKind::Track => return Ok(dedupe(ids.iter().copied())),
        EntityKind::Album => queries::track::track_ids_by_albums(db, ids).await?,
        EntityKind::Artist => queries::track::track_ids_by_artists(db, ids).await?,
        EntityKind::Genre => queries::track::track_ids_by_genres(db, ids).await?,
        EntityKind::Playlist => return playlist_track_ids(db, ids).await,
    };
    Ok(flatten_in_order(group_by_entity(pairs), ids))
}

/// Playlists split two ways in one batched read: manual membership is a second batched read of
/// `playlist_items`, and each smart playlist resolves from its stored rules, a query the database
/// can't union across playlists.
///
/// **Those go out together**, as `library::smart_playlists::fill_from_rules` does, WAL letting them
/// run side by side, and **each asks for ids rather than rows**, the peak being every set at once.
/// An id the split didn't return was deleted since the grid painted, and a rule set that fails is
/// logged; either way the rest of the selection still acts.
async fn playlist_track_ids(db: &DbPool, ids: &[i64]) -> Result<Vec<i64>, AppError> {
    let verdicts = queries::playlist::smart_criteria_for_playlists(db, ids).await?;

    let mut manual_ids: Vec<i64> = Vec::new();
    let mut smart: Vec<(i64, SmartCriteria)> = Vec::new();
    for (id, is_smart, smart_criteria) in verdicts {
        if is_smart {
            smart.push((id, SmartCriteria::from_json_opt(smart_criteria.as_deref())));
        } else {
            manual_ids.push(id);
        }
    }

    let resolved = futures_util::future::join_all(smart.iter().map(|(id, criteria)| async move {
        let track_ids = queries::smart_playlist::get_smart_playlist_track_ids(db, criteria).await;
        (*id, track_ids)
    }))
    .await;

    // Sized for both halves: the manual ones land in the same map through the `extend` below.
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::with_capacity(ids.len());
    for (id, track_ids) in resolved {
        // Logged and skipped, `recount`'s shape.
        match track_ids {
            Ok(track_ids) => {
                grouped.insert(id, track_ids);
            }
            Err(e) => log::warn!("smart playlist {id} resolve failed: {}", describe(&e)),
        }
    }

    let pairs = queries::track::track_ids_by_playlists(db, &manual_ids).await?;
    grouped.extend(group_by_entity(pairs));
    Ok(flatten_in_order(grouped, ids))
}

/// Collect `(entity_id, track_id)` pairs per entity, keeping the order the query returned them in.
fn group_by_entity(pairs: Vec<(i64, i64)>) -> HashMap<i64, Vec<i64>> {
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (entity_id, track_id) in pairs {
        grouped.entry(entity_id).or_default().push(track_id);
    }
    grouped
}

/// Walk the entities in the order the caller asked for and flatten their tracks, dropping repeats.
fn flatten_in_order(mut grouped: HashMap<i64, Vec<i64>>, ids: &[i64]) -> Vec<i64> {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut ordered: Vec<i64> = Vec::new();
    for id in ids {
        for track_id in grouped.remove(id).unwrap_or_default() {
            if seen.insert(track_id) {
                ordered.push(track_id);
            }
        }
    }
    ordered
}

fn dedupe(ids: impl Iterator<Item = i64>) -> Vec<i64> {
    let mut seen: HashSet<i64> = HashSet::new();
    ids.filter(|id| seen.insert(*id)).collect()
}

#[cfg(test)]
#[path = "tests/entity_tracks_tests.rs"]
mod tests;
