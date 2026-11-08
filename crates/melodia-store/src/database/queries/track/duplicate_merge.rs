//! The duplicate finder's Remove: spares out of the library and their history onto the copy kept,
//! in one write transaction, so a removal lands whole or not at all.
//!
//! The library can move while the finder is open, so every copy is read again here, and a removal
//! whose keeper has gone is skipped rather than merged into nothing.

use std::collections::{BTreeSet, HashMap};

use rusqlite::Transaction;

use crate::database::queries::excluded_files::{self, NewExclusion};
use crate::database::queries::ingest::is_bulk;
use crate::database::queries::playlist::{self, NewItem};
use crate::database::queries::{scan, stats};
use crate::database::rows::by_name;
use crate::database::sql::Sql;
use crate::database::{
    DbPool, MAX_BINDS_PER_STATEMENT, chunked_in_execute, chunked_in_query, values_placeholders,
};
use melodia_core::entities::duplicates::GroupRemoval;
use melodia_core::error::AppError;

/// What a removal did, and what moved onto the keepers that the app still owes a follow-up.
#[derive(Debug, Default)]
pub struct RemovedDuplicates {
    pub removed: usize,
    /// Spares already gone since the finder read them, or whose keeper is.
    pub skipped: usize,
    /// Keepers that took a heart from a spare.
    pub hearted: Vec<i64>,
    /// Keepers that took a spare's rating, having none of their own.
    pub rated: Vec<(i64, i32)>,
}

/// A copy as the merge reads it: where its file is, and the history a user built up on it.
#[derive(Clone, Debug)]
struct CopyFacts {
    id: i64,
    file_path: String,
    file_hash: Option<String>,
    file_size: Option<i64>,
    date_modified: Option<String>,
    folder_id: i64,
    play_count: i64,
    skip_count: i64,
    last_played: Option<String>,
    rating: i32,
    is_favorite: bool,
    date_added: String,
}

by_name!(CopyFacts {
    id,
    file_path,
    file_hash,
    file_size,
    date_modified,
    folder_id,
    play_count,
    skip_count,
    last_played,
    rating,
    is_favorite,
    date_added,
});

/// The columns a user builds up on a row, every other one being the file's, which a rescan
/// rewrites. A column joining this list joins [`merged`] too.
#[derive(Debug, PartialEq, Eq)]
struct History {
    is_favorite: bool,
    rating: i32,
    play_count: i64,
    skip_count: i64,
    last_played: Option<String>,
    date_added: String,
}

/// Removes each removal's spares, keeping their files out of later scans and carrying their
/// history onto the keeper.
pub async fn remove_duplicates(
    db: &DbPool,
    removals: Vec<GroupRemoval>,
) -> Result<RemovedDuplicates, AppError> {
    db.transaction(move |tx| merge(tx, &removals)).await
}

fn merge(tx: &Transaction<'_>, removals: &[GroupRemoval]) -> Result<RemovedDuplicates, AppError> {
    let facts = read_facts(tx, removals)?;
    let plan = MergePlan::new(removals, &facts);
    if plan.spares.is_empty() {
        return Ok(plan.outcome);
    }

    // On the bulk path one recalc at the end beats the triggers' per-row cost.
    let bulk = is_bulk(plan.spares.len());
    if bulk {
        stats::disable_stats_triggers(tx)?;
    }
    write_keepers(tx, &plan.keepers)?;
    let touched = move_playlist_places(tx, &plan.keeper_of)?;
    let exclusions: Vec<NewExclusion<'_>> =
        plan.spares.iter().map(|spare| spare.exclusion()).collect();
    excluded_files::exclude_on(tx, &exclusions)?;
    let spare_ids: Vec<i64> = plan.spares.iter().map(|spare| spare.id).collect();
    chunked_in_execute(tx, &spare_ids, |placeholders| {
        format!("DELETE FROM tracks WHERE id IN ({placeholders})")
    })?;
    for playlist_id in touched {
        playlist::renumber_playlist_positions_tx(tx, playlist_id)?;
    }
    scan::roll_up_covers(tx)?;
    scan::prune_orphans(tx)?;
    if bulk {
        stats::recalculate_all_stats(tx)?;
        stats::enable_stats_triggers(tx)?;
    }
    Ok(plan.outcome)
}

fn read_facts(
    tx: &Transaction<'_>,
    removals: &[GroupRemoval],
) -> Result<HashMap<i64, CopyFacts>, AppError> {
    let ids: Vec<i64> = removals
        .iter()
        .flat_map(|removal| std::iter::once(removal.keeper).chain(removal.spares.iter().copied()))
        .collect();
    let rows: Vec<CopyFacts> = chunked_in_query(tx, &ids, |placeholders| {
        format!(
            "SELECT id, file_path, file_hash, file_size, date_modified, folder_id, play_count, \
                    skip_count, last_played, rating, is_favorite, date_added \
             FROM tracks WHERE id IN ({placeholders})"
        )
    })?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

/// What the transaction writes, decided before it writes anything.
struct MergePlan<'a> {
    keepers: Vec<(i64, History)>,
    keeper_of: HashMap<i64, i64>,
    spares: Vec<&'a CopyFacts>,
    outcome: RemovedDuplicates,
}

impl<'a> MergePlan<'a> {
    fn new(removals: &[GroupRemoval], facts: &'a HashMap<i64, CopyFacts>) -> Self {
        let mut plan = Self {
            keepers: Vec::with_capacity(removals.len()),
            keeper_of: HashMap::new(),
            spares: Vec::new(),
            outcome: RemovedDuplicates::default(),
        };
        for removal in removals {
            let Some(keeper) = facts.get(&removal.keeper) else {
                plan.outcome.skipped += removal.spares.len();
                continue;
            };
            let spares: Vec<&CopyFacts> =
                removal.spares.iter().filter_map(|id| facts.get(id)).collect();
            plan.outcome.skipped += removal.spares.len() - spares.len();
            if spares.is_empty() {
                continue;
            }
            plan.take(keeper, spares);
        }
        plan.outcome.removed = plan.spares.len();
        plan
    }

    fn take(&mut self, keeper: &CopyFacts, spares: Vec<&'a CopyFacts>) {
        let history = merged(keeper, &spares);
        if history.is_favorite && !keeper.is_favorite {
            self.outcome.hearted.push(keeper.id);
        }
        if history.rating != keeper.rating {
            self.outcome.rated.push((keeper.id, history.rating));
        }
        self.keepers.push((keeper.id, history));
        self.keeper_of.extend(spares.iter().map(|spare| (spare.id, keeper.id)));
        self.spares.extend(spares);
    }
}

/// The keeper's history with its spares' folded in.
///
/// A rating stays the keeper's own where it has one: a tag rating wins on rescan, so a merged one
/// would be overruled by the keeper's file. Added is the earliest, the song having been in the
/// library since then.
fn merged(keeper: &CopyFacts, spares: &[&CopyFacts]) -> History {
    let copies = || std::iter::once(keeper).chain(spares.iter().copied());
    let spare_rating = spares.iter().map(|spare| spare.rating).max().unwrap_or(0);
    History {
        is_favorite: copies().any(|copy| copy.is_favorite),
        rating: if keeper.rating > 0 { keeper.rating } else { spare_rating },
        play_count: copies().map(|copy| copy.play_count).sum(),
        skip_count: copies().map(|copy| copy.skip_count).sum(),
        last_played: copies().filter_map(|copy| copy.last_played.clone()).max(),
        date_added: copies()
            .map(|copy| &copy.date_added)
            .min()
            .unwrap_or(&keeper.date_added)
            .clone(),
    }
}

impl CopyFacts {
    fn exclusion(&self) -> NewExclusion<'_> {
        // Unstamped without a hash, so the next scan reads the file and fills one in.
        let stamped = self.file_hash.is_some();
        NewExclusion {
            path: &self.file_path,
            file_hash: self.file_hash.as_deref(),
            file_size: self.file_size.filter(|_| stamped),
            date_modified: self.date_modified.as_deref().filter(|_| stamped),
            folder_id: self.folder_id,
        }
    }
}

fn write_keepers(tx: &Transaction<'_>, keepers: &[(i64, History)]) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 7;
    for chunk in keepers.chunks(MAX_BINDS_PER_STATEMENT / COLS_PER_ROW) {
        let rows = values_placeholders(chunk.len(), COLS_PER_ROW);
        let mut sql = Sql::trusted(format!(
            "WITH v(id, is_favorite, rating, play_count, skip_count, last_played, date_added) \
                 AS (VALUES {rows}) \
             UPDATE tracks SET is_favorite = v.is_favorite, rating = v.rating, \
                    play_count = v.play_count, skip_count = v.skip_count, \
                    last_played = v.last_played, date_added = v.date_added \
             FROM v WHERE tracks.id = v.id"
        ));
        for (id, history) in chunk {
            sql.bind(*id)
                .bind(history.is_favorite)
                .bind(history.rating)
                .bind(history.play_count)
                .bind(history.skip_count)
                .bind(history.last_played.as_deref())
                .bind(history.date_added.as_str());
        }
        sql.execute(tx)?;
    }
    Ok(())
}

/// Gives each keeper its spares' playlist places, at its first spare's position and with that
/// spare's added date, returning the playlists touched. An insert rather than a re-point, since
/// the playlist stats triggers watch only inserts and deletes; a playlist the keeper is already in
/// keeps the keeper's own place.
fn move_playlist_places(
    tx: &Transaction<'_>,
    keeper_of: &HashMap<i64, i64>,
) -> Result<BTreeSet<i64>, AppError> {
    let spare_ids: Vec<i64> = keeper_of.keys().copied().collect();
    let mut places: Vec<(i64, i64, i32, String)> =
        chunked_in_query(tx, &spare_ids, |placeholders| {
            format!(
                "SELECT playlist_id, track_id, position, added_at FROM playlist_items \
                 WHERE track_id IN ({placeholders})"
            )
        })?;
    // Lowest first, since the first insert wins; in Rust, as an ORDER BY holds per chunk only.
    places.sort_unstable_by_key(|&(_, _, position, _)| position);

    let mut by_playlist: HashMap<i64, Vec<NewItem<'_>>> = HashMap::new();
    for (playlist_id, spare_id, position, added_at) in &places {
        let Some(&keeper) = keeper_of.get(spare_id) else {
            continue;
        };
        by_playlist.entry(*playlist_id).or_default().push(NewItem {
            track_id: keeper,
            position: *position,
            added_at,
        });
    }
    for (playlist_id, items) in &by_playlist {
        playlist::insert_items_tx(tx, *playlist_id, items)?;
    }
    Ok(by_playlist.into_keys().collect())
}

#[cfg(test)]
#[path = "../tests/duplicate_merge_tests.rs"]
mod tests;
