//! Remove: the picked spares out of the library, their history onto the copies kept. No file is
//! touched, so the watcher sees nothing and the removal is the store's one transaction.

use std::collections::HashMap;

use crate::library::{favorites, ratings};
use crate::state::AppState;
use crate::tasks::{self, TaskSpawner};
use melodia_core::entities::duplicates::GroupRemoval;
use melodia_core::error::AppError;
use melodia_engine::player::engine::actions::emit_and_execute;
use melodia_store::database::{DbPool, queries};

/// What a Remove did, for its result line.
#[derive(Clone, Copy, Debug)]
pub struct RemovalCounts {
    pub removed: usize,
    /// Picked copies passed by: the playing one, and those that, or whose keeper, left the library
    /// since the find.
    pub skipped: usize,
}

/// Takes each removal's spares out of the library, leaving their files where they are.
///
/// A merge that fails leaves the queue swapped, each swapped row on its keeper: a copy of the same
/// song, still in the library.
pub async fn apply(
    state: &AppState,
    mut removals: Vec<GroupRemoval>,
) -> Result<RemovalCounts, AppError> {
    let referenced_before = queries::artwork::referenced_filenames(&state.db).await?;
    let orphaned = drop_orphaned(&state.db, &mut removals).await?;
    let held = hold_back_playing_and_requeue(state, &mut removals);
    // After the hold-back no spare can start, so each count one earned lands before its row goes.
    state.play_count_flush.flush().await;

    let removed = queries::track::remove_duplicates(&state.db, removals).await?;
    if removed.removed > 0 {
        state.library_changed.bump();
        // Counts moved onto the keepers, which re-rank what sorts by plays.
        state.stats_changed.bump();
        tasks::artwork_sweep::retire_released(
            &TaskSpawner::from_state(state),
            state,
            referenced_before,
        );
    }
    favorites::merged_onto(state, &removed.hearted).await;
    ratings::merged_onto(state, &removed.rated);
    Ok(RemovalCounts { removed: removed.removed, skipped: removed.skipped + held + orphaned })
}

/// Drops the removals whose keeper left the library since the find, answering how many copies
/// they picked: requeued onto a keeper that is gone, a spare's rows would be pruned while the
/// spare itself stays.
async fn drop_orphaned(db: &DbPool, removals: &mut Vec<GroupRemoval>) -> Result<usize, AppError> {
    let keepers: Vec<i64> = removals.iter().map(|removal| removal.keeper).collect();
    let gone = queries::track::missing_ids(db, &keepers).await?;
    let orphaned = removals.extract_if(.., |removal| gone.contains(&removal.keeper));
    Ok(orphaned.map(|removal| removal.spares.len()).sum())
}

/// Holds the playing copy back from its removal and swaps every other queued spare for its
/// keeper, in one step under the player lock, answering how many copies were held.
///
/// A play the playing copy finishes counts against its own id, which the delete would take.
///
/// Under the exec lock, which an advance holds until it has sent the count for the track it left,
/// so a spare that just stopped playing has its count in the flusher's channel before this runs.
fn hold_back_playing_and_requeue(state: &AppState, removals: &mut [GroupRemoval]) -> usize {
    let mut held = 0;
    emit_and_execute(&*state.engine, &state.player_state, &state.sinks, |s| {
        let playing = s.current_track().map(|track| track.id);
        let mut keeper_of = HashMap::new();
        for removal in removals.iter_mut() {
            let picked = removal.spares.len();
            removal.spares.retain(|&spare| Some(spare) != playing);
            held += picked - removal.spares.len();
            keeper_of.extend(removal.spares.iter().map(|&spare| (spare, removal.keeper)));
        }
        s.queue.replace_ids(&keeper_of);
        Vec::new()
    });
    held
}
