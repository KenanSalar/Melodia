//! The tracks behind a set of entity cards — what every card right-click action operates on.
//!
//! One place that knows how an album, artist, genre or playlist becomes a track list, so the eight
//! `CardActions` handlers each resolve through a single call rather than repeating the match.
//! Browse's folder cards are deliberately absent: a folder is a path rather than an id, and the
//! concept belongs to [`crate::library::browse`].

use std::collections::{HashMap, HashSet};

use crate::state::AppState;
use melodia_core::entities::smart_criteria::SmartCriteria;
use melodia_core::error::{AppError, describe};
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

/// Playlists split two ways, and which side an id falls on is one batched read: manual membership
/// is a second batched read of `playlist_items`, while a smart playlist has no rows there at all
/// and is resolved from its stored criteria, one query each. Only that last part is per-entity,
/// a stored rule set being a query the database cannot union across playlists.
///
/// **Those go out together**, `library::smart_playlists::recount`'s shape: WAL lets them run side
/// by side across the read pool, where awaiting each in turn put a grid's worth of them in series
/// on the click path. **Each projects to ids before the join collects it**, the membership query
/// answering in full list rows: held whole, a selection's peak is every matched row of every
/// playlist in it, where the serial version it replaced held one set at a time.
///
/// An id the split read didn't return was deleted between the grid painting and the click, and a
/// rule set the evaluator can't answer is logged where it fails. Either way the rest of the
/// selection still acts: a menu that resolves nothing has only a log line to show for it.
async fn playlist_track_ids(state: &AppState, ids: &[i64]) -> Result<Vec<i64>, AppError> {
    let verdicts = queries::playlist::smart_criteria_for_playlists(&state.db, ids).await?;

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
        let track_ids = queries::smart_playlist::get_smart_playlist_tracks(&state.db, criteria)
            .await
            .map(|rows| rows.into_iter().map(|row| row.id).collect::<Vec<i64>>());
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
