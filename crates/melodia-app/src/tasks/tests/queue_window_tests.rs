//! What the window task fetches: on a queue move the records around the current slot that the
//! window lacks, and on a library tick every record it holds, read again.

use super::*;
use crate::state::fixtures::TestPlayback;
use melodia_core::error::AppError;
use melodia_engine::player::engine::state::{lock_state, with_state_emit};

/// A fixture with `count` staged tracks queued from the head and no record in the window, as a
/// queue edit the task has yet to answer leaves it.
async fn queued_unfetched(count: usize) -> Result<(TestPlayback, Vec<i64>), AppError> {
    let fx = TestPlayback::empty().await?;
    let ids = fx.stage_playable(count).await?;
    with_state_emit(&fx.ctx.player_state, &fx.ctx.sinks, |s| {
        s.queue.add_tracks(&ids);
        s.queue.current_index = Some(0);
    });
    Ok((fx, ids))
}

async fn fill_window_of(fx: &TestPlayback, fill: WindowFill) -> AppResult<()> {
    fill_window(&fx.ctx.db, &fx.ctx.player_state, &fx.ctx.sinks, fill).await
}

/// The title of the record the window holds for `id`, if it holds one.
fn held_title(fx: &TestPlayback, id: i64) -> Option<String> {
    lock_state(&fx.ctx.player_state).window.get(id).map(|record| record.title.clone())
}

/// A queue move fetches the records a transition can move onto, the current slot and the three
/// after it, and none past them: fetching further is the per-track cost the queue holding ids
/// exists to avoid.
#[tokio::test]
async fn a_queue_move_fetches_the_records_around_the_current_slot_and_no_further()
-> Result<(), AppError> {
    let (fx, ids) = queued_unfetched(5).await?;

    fill_window_of(&fx, WindowFill::Missing).await?;

    let held: Vec<bool> = ids.iter().map(|&id| held_title(&fx, id).is_some()).collect();
    assert_eq!(held, [true, true, true, true, false]);
    Ok(())
}

/// A retag or a star on a queued track has to show once it plays, and only a library tick can say
/// one landed, so only that fill reads a record the window already holds.
#[tokio::test]
async fn only_a_library_tick_reads_a_held_record_again() -> Result<(), AppError> {
    let rows = [(WindowFill::Missing, "Track 2"), (WindowFill::All, "Retagged")];
    for (wake, expected) in rows {
        let (fx, ids) = queued_unfetched(3).await?;
        fill_window_of(&fx, WindowFill::Missing).await?;
        sqlx::query("UPDATE tracks SET title = 'Retagged' WHERE id = ?")
            .bind(ids[1])
            .execute(fx.ctx.db.write())
            .await?;

        fill_window_of(&fx, wake).await?;

        assert_eq!(held_title(&fx, ids[1]).as_deref(), Some(expected), "{wake:?}");
    }
    Ok(())
}
