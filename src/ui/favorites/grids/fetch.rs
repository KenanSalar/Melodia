//! The two grid queries, and what happens between them landing and the cards
//! appearing.

use std::sync::Arc;

use slint::Weak;

use super::apply::apply_filtered_grids;
use super::sort::sort_cached_artists;
use crate::AppWindow;
use crate::library;
use crate::state::AppState;
use crate::ui::favorites::FavoritesUi;

/// Fetch Most Played + Favorite Artists in parallel and apply each
/// independently. Returns `()` because callers (`kick_full_refresh`)
/// have no use for a propagated error — every failure is logged here
/// with section context.
///
/// `tokio::join!`, **not** `try_join!`: a failure on one tab must not suppress
/// the other. A previous `try_join!` here silently hid both surfaces whenever
/// the Albums query errored, which was the root cause of "the artists section
/// never showed up" before the album section was dropped.
pub async fn refresh_grids(state: &AppState, fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let (most_played_res, fav_artists_res) = tokio::join!(
        library::favorites::get_most_played_favorites(state),
        library::favorites::get_favorite_artists(state),
    );

    // Logged before the guard below, not at the store — a query that failed is
    // worth a line whether or not anyone is still looking at the view.
    let most_played = most_played_res
        .inspect_err(|e| log::warn!("favorites::refresh_grids most_played: {e}"))
        .ok();
    let fav_artists = fav_artists_res
        .inspect_err(|e| log::warn!("favorites::refresh_grids fav_artists: {e}"))
        .ok();

    // A leave that landed while the two queries were in flight has already
    // cleared these caches (and emptied both models), so storing now would undo
    // the teardown behind a view nobody can see. Nothing is lost by dropping the
    // result: every leave sets `mark_dirty`, so the next enter re-fetches.
    if !fav_ui.section_active() {
        return;
    }

    // Folded here rather than at publish time, for the reason `refresh_tracks`
    // gives: this is the worker holding the rows, and the UI thread has no
    // business summing a play history inside an `upgrade_in_event_loop`. Ahead
    // of the gate, for the reason it gives too — the walk isn't a store, and the
    // wipe queues behind whatever the gate is held across.
    let most_played = most_played.map(|rows| (crate::ui::hero_chips::fold_most_played(&rows), rows));

    // Serialize the stores against `release_section_state`'s wipe through the
    // section gate, the way `albums::grid::fetch_grid` does: without it a fast
    // leave → re-enter can let the wipe land *between* this fresh-data write
    // and the repaint below, and the grid paints empty. Held only across the
    // synchronous stores — never across an `.await`.
    {
        let _gate = fav_ui.gate();
        if let Some((totals, rows)) = most_played {
            *fav_ui.state().most_played_totals.lock() = totals;
            *fav_ui.state().most_played.lock() = rows;
        }
        if let Some(rows) = fav_artists {
            *fav_ui.state().fav_artists.lock() = rows;
            // Before the prewarm below, which reads this cache for its paths —
            // the query returns no order at all, so this is what puts the rows
            // in the one the tab is about to paint.
            sort_cached_artists(fav_ui);
        }
    }
    // The grid write below publishes too, but only past a signature taken over
    // the *filtered* rows — so a play-count bump on a track the filter excludes
    // moves the totals the band states without moving that hash, and on the
    // Songs tab `mounted_content` is a constant `0` besides.
    crate::ui::favorites::hero::republish_chips(fav_ui, weak);

    // Prewarm the mounted tab's tier off-thread before its rows land in the
    // Slint model: the cards' `request-*-cover` lookups decode on miss *on
    // the UI thread*, so a cold tab would otherwise pay one synchronous
    // 448 px decode per visible card at first paint.
    //
    // Only the mounted tab, and only while the section is on screen. The two
    // grids are mutually exclusive, so warming both is twice the decodes and
    // twice the resident buffers for a surface nobody can scroll; and a
    // `library_changed` tick that arrives while Favorites is hidden has
    // already been turned into a `mark_dirty` by the caller, so there is
    // nothing on screen to warm for. The other tab warms in `tab-changed`.
    //
    // `Some(tab)` only once the decode reports the tier is still *its* — a
    // leave landing inside the burst hands the buffers back, and announcing
    // that tab anyway would bump `covers-generation` over an empty tier on the
    // re-enter. A `JoinError` is the same "we don't know", so it folds in here.
    let warmed_tab = if fav_ui.section_active() {
        let fu = fav_ui.clone();
        let tab = fav_ui.active_tab();
        let warm = tokio::task::spawn_blocking(move || fu.prewarm_tab_covers(tab)).await;
        matches!(warm, Ok(true)).then_some(tab)
    } else {
        None
    };

    // Both fetches resolved (or logged); push the filtered model so the
    // visible tab reflects fresh data AND the live filter in one pass.
    apply_filtered_grids(fav_ui, weak, warmed_tab);
}
