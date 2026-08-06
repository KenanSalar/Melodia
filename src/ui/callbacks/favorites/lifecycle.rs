//! Favorites section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch), the `library_changed` subscriber,
//! and the first-enter initial fetch. See [`super::wire_favorites`].

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, SharedString};

use super::NAV_FAVORITES;
use crate::state::AppState;
use crate::ui::callbacks::macros::release_shared_hero;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::model_diff::clear_vec_model;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::{
    AppWindow, EntityGridRow as UiEntityGridRow, Favorites, Nav, TrackListRow as UiTrackListRow,
};

/// Wire the Favorites section-lifecycle callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // --- Section-active mirror + cache release / re-enter --------
    // Seed the synchronous shadow from the current nav state. This has to be
    // right on its own: the gate's `ChangeTracker` baselines inside
    // `AppWindow::new()` and fires only on a later difference, so a section
    // the boot doesn't land on gets no edge at all, and the one it does land
    // on gets its edge a frame late — after boot has already read this
    // shadow. See the `SectionActiveGate` bullet in
    // `.claude/rules/ui-patterns.md`. `boot::ui_setup::install_views`
    // hydrates the persisted nav index before any `wire_*` runs, so the read
    // below sees it.
    // (The sibling `active_tab` shadow is seeded by `favorites::seed_tab`,
    // which runs after this and is the only thing that knows the persisted
    // value.)
    fav_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_FAVORITES);
    {
        let fu = fav_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            fu.set_section_active(active);
            if !active {
                // Land synchronously on the UI thread *before* the
                // release task is even spawned — the re-enter handler
                // reads this via `take_dirty()` and must never observe
                // stale data because release hadn't run yet. Mirrors
                // `AlbumsUi::data_dirty`.
                fu.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                // UI-thread teardown: clear Slint Image properties so
                // the `SharedPixelBuffer` Arcs the LRU is about to
                // clear release immediately (the dual-slot blur slots
                // hold their own refs even after the LRU drops). Empty
                // the grid + tracks models so their `SharedString`s
                // also drop on the same tick.
                let g = ui.global::<Favorites>();
                g.set_blur_img_a(Image::default());
                g.set_blur_img_b(Image::default());
                g.set_has_blur(false);
                // Same tick as the wipe above, and unconditional for the same
                // reason: `release_section_state` bails out when the user has
                // already come back, so leaving the guard to it can strand the
                // hero on the bare gradient floor until the next channel tick.
                fu.forget_mosaic();
                // Same tick, same reason: the models are emptied below, so a
                // surviving signature would match the identical data on
                // re-enter and skip the refill that fills them back in.
                fu.forget_grid_signature();
                // Six heroes share one colour set and one chip row, so hand
                // both back rather than leaving this mosaic's solve and this
                // tab's counts for the next hero to paint under.
                release_shared_hero!(ui);
                // Both grid tiers go with `release_section_state` below, so
                // rewind the counter that means "cold" — else the next enter
                // reads a leftover bump as a warm tier and decodes on mount.
                g.set_covers_generation(0);
                // And rewind all three counts to "not fetched yet" on the same
                // tick as the models they number, for the reason the folds are
                // reset beside their caches: a count that outlives its model is
                // the one thing these surfaces can state that is *wrong* rather
                // than merely absent. `track-count` is the visible one — the
                // hero square reads it, so a stale non-zero drew four
                // placeholder mosaic slots over an emptied `mosaic-paths` until
                // the re-enter fetch landed.
                g.set_track_count(UNFETCHED_COUNT);
                g.set_most_played_count(UNFETCHED_COUNT);
                g.set_artist_count(UNFETCHED_COUNT);
                clear_vec_model::<UiTrackListRow>(&g.get_tracks(), "favorites: clear tracks");
                clear_vec_model::<UiEntityGridRow>(
                    &g.get_most_played_rows(),
                    "favorites: clear most-played",
                );
                clear_vec_model::<UiEntityGridRow>(&g.get_artist_rows(), "favorites: clear artist");
                clear_vec_model::<i32>(&g.get_selected_ids(), "favorites: clear selected-ids");
                clear_vec_model::<SharedString>(
                    &g.get_mosaic_paths(),
                    "favorites: clear mosaic-paths",
                );
                g.set_selection_anchor(-1);
            }
            let fu = fu.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if fu.take_dirty() {
                        kick_full_refresh(&s, &fu, &weak).await;
                    }
                });
            } else {
                // Off-thread heavy release — runs after UI-thread
                // teardown so the LRU drop pairs with already-released
                // Slint properties (no lingering Arc refs).
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || fu.release_section_state());
            }
        });
    }

    // --- library_changed_tx + stats_changed_tx subscriber ---
    // `library_changed` is bumped after every `set_favorite` /
    // `toggle_current_favorite` + every scan / file-event commit;
    // `stats_changed` after every play-count flush. Favorites is
    // the only surface that ranks by `play_count` (hero mosaic + the Most
    // Played tab), so it alone listens to both channels. While the
    // Favorites section is visible we refetch hero + grids + tracks
    // in-place; while hidden we just mark dirty so the next enter
    // triggers `kick_full_refresh`.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        let mut library_rx = state.library_changed_tx.subscribe();
        let mut stats_rx = state.stats_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            library_rx.mark_unchanged();
            stats_rx.mark_unchanged();
            loop {
                // Both senders live in `AppState` for the process lifetime,
                // so an `Err` only happens during teardown — exit the loop.
                tokio::select! {
                    changed = library_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    changed = stats_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
                if !fu.section_active() {
                    fu.mark_dirty();
                    continue;
                }
                kick_full_refresh(&s, &fu, &weak).await;
            }
        }));
    }

    // First section enter: kick an initial fetch so the page paints
    // with data instead of empty models when the user lands here. If
    // the session starts on a different tab, `section-active-changed`
    // will trigger the same fetch via `take_dirty()` (mark_dirty in
    // the `else` arm above would never fire on a session that *never*
    // visited Favorites). For the start-on-Favorites case, mark dirty
    // synchronously here so the subsequent `section-active-changed`
    // fire (or the next library_changed tick) re-fetches.
    fav_ui.mark_dirty();
    if fav_ui.section_active() {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        state.runtime.clone().spawn(async move {
            if fu.take_dirty() {
                kick_full_refresh(&s, &fu, &weak).await;
            }
        });
    }
}

/// Fetch hero stats + the two grid tabs + the Songs list and apply each
/// as it lands. Concurrent — `tokio::join!` runs all three in
/// parallel. `refresh_grids` already logs its own per-tab errors
/// (because Most Played + Artists are applied independently), so it
/// returns `()`; the other two return `AppResult<()>` and have their
/// errors logged here.
async fn kick_full_refresh(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &slint::Weak<AppWindow>,
) {
    let (h, _grids, t) = tokio::join!(
        favorites_ui_mod::refresh_hero(state, fav_ui, weak, /* animate */ true),
        favorites_ui_mod::refresh_grids(state, fav_ui, weak),
        favorites_ui_mod::refresh_tracks(state, fav_ui, weak),
    );
    if let Err(e) = h {
        log::warn!("favorites::refresh_hero: {e}");
    }
    if let Err(e) = t {
        log::warn!("favorites::refresh_tracks: {e}");
    }
}
