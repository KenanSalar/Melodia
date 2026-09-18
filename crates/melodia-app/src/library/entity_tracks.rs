//! The tracks behind a set of entity cards — what every card right-click action operates on.
//!
//! One place that knows how an album, artist, genre or playlist becomes a track list, so the nine
//! `CardActions` handlers each resolve through a single call rather than repeating the match.
//! Browse's folder cards are deliberately absent: a folder is a path rather than an id, and the
//! concept belongs to [`crate::library::browse`].

use std::collections::{HashMap, HashSet};

use crate::state::AppState;
use melodia_core::entities::smart_criteria::SmartCriteria;
use melodia_core::error::AppError;
use melodia_store::database::queries;

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
/// The dedupe is load-bearing rather than tidy — a track credited to two selected artists, or
/// sitting in two selected playlists, should be queued once.
pub async fn track_ids_for(
    state: &AppState,
    kind: EntityKind,
    ids: &[i64],
) -> Result<Vec<i64>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let pairs = match kind {
        EntityKind::Track => return Ok(dedupe(ids.iter().copied())),
        EntityKind::Album => queries::track::track_ids_by_albums(&state.db, ids).await?,
        EntityKind::Artist => queries::track::track_ids_by_artists(&state.db, ids).await?,
        EntityKind::Genre => queries::track::track_ids_by_genres(&state.db, ids).await?,
        EntityKind::Playlist => return playlist_track_ids(state, ids).await,
    };
    Ok(flatten_in_order(group_by_entity(pairs), ids))
}

/// Playlists split two ways: manual membership is one batched read of `playlist_items`, while a
/// smart playlist has no rows there at all and is resolved from its stored criteria, one query
/// each. A selection here is a handful of cards, so the per-smart-playlist query isn't worth
/// batching away.
async fn playlist_track_ids(state: &AppState, ids: &[i64]) -> Result<Vec<i64>, AppError> {
    let mut manual_ids: Vec<i64> = Vec::new();
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();

    for &id in ids {
        let detail = match queries::playlist::get_playlist_by_id(&state.db, id).await {
            Ok(detail) => detail,
            // Deleted between the grid painting and the click. The rest of the selection still acts.
            Err(AppError::NotFound(_)) => continue,
            Err(e) => return Err(e),
        };
        if !detail.is_smart {
            manual_ids.push(id);
            continue;
        }
        let criteria = SmartCriteria::from_json_opt(detail.smart_criteria.as_deref());
        let rows = queries::smart_playlist::get_smart_playlist_tracks(&state.db, &criteria).await?;
        grouped.insert(id, rows.into_iter().map(|row| row.id).collect());
    }

    let pairs = queries::track::track_ids_by_playlists(&state.db, &manual_ids).await?;
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
