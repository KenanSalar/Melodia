//! The Most Played query, and what happens between it landing and the cards
//! appearing.

use std::sync::Arc;

use slint::Weak;

use super::apply::apply_filtered_grid;
use crate::AppWindow;
use crate::library;
use crate::state::AppState;
use crate::ui::recently_played::RecentlyPlayedUi;

/// Fetch the library's played tracks and apply them (filtered). Returns `()` —
/// the caller (`kick_full_refresh`) has no use for a propagated error; failures
/// are logged here with section context.
pub async fn refresh_grid(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
) {
    // Logged before the guard below, not at the store — a query that failed is
    // worth a line whether or not anyone is still looking at the view.
    let most_played = library::recently_played::get_most_played(state)
        .await
        .inspect_err(|e| log::warn!("recently_played::refresh_grid most_played: {e}"))
        .ok();

    // A leave that landed while the query was in flight has already cleared this
    // cache (and emptied the model), so storing now would undo the teardown
    // behind a view nobody can see. Nothing is lost by dropping the result: every
    // leave sets `mark_dirty`, so the next enter re-fetches.
    if !rp_ui.section_active() {
        return;
    }

    // Folded here rather than at publish time, for the reason `songs::refresh_tracks`
    // gives: this is the worker holding the rows, and the UI thread has no
    // business summing a play history inside an `invoke_from_event_loop`. Ahead
    // of the gate, for the reason it gives too — the walk isn't a store, and the
    // wipe queues behind whatever the gate is held across.
    let most_played =
        most_played.map(|rows| (crate::ui::hero_chips::fold_most_played(&rows), rows));

    // Serialize the store against `release_section_state`'s wipe through the
    // section gate: without it a fast leave → re-enter can let the wipe land
    // *between* this fresh-data write and the repaint below, and the grid paints
    // empty. Held only across the synchronous stores — never across an `.await`.
    {
        let _gate = rp_ui.gate();
        if let Some((totals, rows)) = most_played {
            *rp_ui.state().most_played_totals.lock() = totals;
            *rp_ui.state().most_played.lock() = rows;
        }
    }
    // The grid write below publishes too, but only past a signature taken over
    // the *filtered* rows — so a play-count bump on a track the filter excludes
    // moves the totals the band states without moving that hash, and on the Songs
    // tab `mounted_content` is a constant `0` besides.
    crate::ui::recently_played::hero::republish_chips(rp_ui, weak);

    // Prewarm the tier off-thread before its rows land in the Slint model: the
    // cards' `request-most-played-cover` lookups decode on miss *on the UI
    // thread*, so a cold tab would otherwise pay one synchronous 448 px decode
    // per visible card at first paint.
    //
    // Only the mounted tab, and only while the section is on screen. A
    // `stats_changed` tick that arrives while the page is hidden has already been
    // turned into a `mark_dirty` by the caller, so there is nothing on screen to
    // warm for; the tab warms in `tab-changed` instead.
    //
    // `Some(tab)` only once the decode reports the tier is still *its* — a leave
    // landing inside the burst hands the buffers back, and announcing that tab
    // anyway would bump `covers-generation` over an empty tier on the re-enter. A
    // `JoinError` is the same "we don't know", so it folds in here.
    let warmed_tab = if rp_ui.section_active() {
        let ru = rp_ui.clone();
        let tab = rp_ui.active_tab();
        let warm = tokio::task::spawn_blocking(move || ru.prewarm_tab_covers(tab)).await;
        matches!(warm, Ok(true)).then_some(tab)
    } else {
        None
    };

    // Push the filtered model so the visible tab reflects fresh data AND the live
    // filter in one pass.
    apply_filtered_grid(rp_ui, weak, warmed_tab);
}
